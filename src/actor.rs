// SPDX-License-Identifier: MIT
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, trace, warn};

#[cfg(feature = "metrics")]
use metrics::counter;

use crate::error::HandlerResult;
use crate::types::{BusConfig, DeadLetter, FailurePolicy, SubscriptionId};

#[cfg(feature = "metrics")]
use crate::metrics::TimerGuard;

/// Type alias for the boxed future returned by async handlers.
///
/// Each async handler invocation allocates one `Box::pin(Future)` — this is
/// the cost of type-erased async dispatch and cannot be avoided without
/// monomorphisation across all handler types.
pub(crate) type HandlerFuture = Pin<Box<dyn Future<Output = HandlerResult> + Send>>;

/// Events are wrapped in `Arc` once at publish time and shared by reference
/// across all listeners.  Async listeners clone the `Arc` (cheap ref-count
/// bump); sync listeners receive `&dyn Any` with zero additional allocation.
pub(crate) type EventType = Arc<dyn Any + Send + Sync>;
pub(crate) type ErasedAsyncHandler = Arc<dyn Fn(EventType) -> HandlerFuture + Send + Sync + 'static>;
pub(crate) type ErasedSyncHandler = Arc<dyn Fn(&(dyn Any + Send + Sync)) -> HandlerResult + Send + Sync + 'static>;

/// Type-erased handler that preserves the sync/async dispatch mode.
#[derive(Clone)]
pub(crate) enum ErasedHandler {
    Async(ErasedAsyncHandler),
    Sync(ErasedSyncHandler),
}

impl ErasedHandler {
    /// Returns `true` if this is a synchronous handler.
    pub(crate) fn is_sync(&self) -> bool {
        matches!(self, Self::Sync(_))
    }
}

#[derive(Clone)]
pub(crate) struct Listener {
    pub id: SubscriptionId,
    pub handler: ErasedHandler,
    pub failure_policy: FailurePolicy,
}

#[derive(Debug)]
struct ListenerFailure {
    event_name: &'static str,
    subscription_id: SubscriptionId,
    attempts: usize,
    error: String,
    dead_letter: bool,
}

type HandlerTaskOutcome = Option<ListenerFailure>;

pub(crate) enum BusMessage {
    Subscribe {
        event_type: TypeId,
        handler: ErasedHandler,
        failure_policy: FailurePolicy,
        ack: oneshot::Sender<SubscriptionId>,
    },
    Unsubscribe {
        subscription_id: SubscriptionId,
        ack: oneshot::Sender<bool>,
    },
    Publish {
        event_type: TypeId,
        event: EventType,
        event_name: &'static str,
        ack: Option<oneshot::Sender<()>>,
    },
    Shutdown {
        ack: oneshot::Sender<Result<(), crate::error::EventBusError>>,
    },
}

impl BusMessage {
    pub(crate) fn operation_name(&self) -> &'static str {
        match self {
            Self::Subscribe { .. } => "subscribe",
            Self::Unsubscribe { .. } => "unsubscribe",
            Self::Publish { .. } => "publish",
            Self::Shutdown { .. } => "shutdown",
        }
    }
}

pub(crate) struct EventBusActor {
    tx: mpsc::Sender<BusMessage>,
    rx: mpsc::Receiver<BusMessage>,
    /// Copy-on-write listener lists.
    ///
    /// Each event type maps to an `Arc<Vec<Listener>>`.  During dispatch the
    /// `Arc` is cheaply cloned (O(1)), allowing iteration without borrowing
    /// `&self` mutably.  Mutations (subscribe/unsubscribe) go through
    /// `Arc::make_mut`, which clones the inner `Vec` only when other clones
    /// are still alive — giving us copy-on-write semantics without manual
    /// reference counting.
    listeners: HashMap<TypeId, Arc<Vec<Listener>>>,
    listener_index: HashMap<SubscriptionId, TypeId>,
    next_subscription_id: u64,
    async_tasks: JoinSet<HandlerTaskOutcome>,
    handler_timeout: Option<Duration>,
    async_semaphore: Option<Arc<Semaphore>>,
    shutdown_timeout: Option<Duration>,
}

impl EventBusActor {
    pub fn new(tx: mpsc::Sender<BusMessage>, rx: mpsc::Receiver<BusMessage>, config: &BusConfig) -> Self {
        let async_semaphore = config.max_concurrent_async.map(|n| Arc::new(Semaphore::new(n)));
        Self {
            tx,
            rx,
            listeners: HashMap::new(),
            listener_index: HashMap::new(),
            next_subscription_id: 1,
            async_tasks: JoinSet::new(),
            handler_timeout: config.handler_timeout,
            async_semaphore,
            shutdown_timeout: config.shutdown_timeout,
        }
    }

    pub async fn run(mut self) {
        trace!("event_bus_actor.run");
        loop {
            // Use select! to concurrently poll the message channel and in-flight
            // async tasks.  This ensures that async handler failures and dead
            // letters are processed promptly even when the bus is idle.
            tokio::select! {
                biased;

                msg = self.rx.recv() => {
                    let Some(msg) = msg else {
                        // Channel closed without an explicit Shutdown message.
                        break;
                    };

                    // Eagerly reap completed async tasks before processing the
                    // message.  Under sustained publish load the `biased`
                    // select always favours the mailbox, so this ensures dead
                    // letters and task completions are not starved.
                    self.reap_finished_async_tasks();

                    match msg {
                        BusMessage::Subscribe {
                            event_type,
                            handler,
                            failure_policy,
                            ack,
                        } => {
                            let id = self.next_subscription_id();
                            let list = self.listeners.entry(event_type).or_default();
                            Arc::make_mut(list).push(Listener {
                                id,
                                handler,
                                failure_policy,
                            });
                            self.listener_index.insert(id, event_type);

                            if let Err(_e) = ack.send(id) {
                                warn!("subscribe.ack_receiver_dropped");
                            }
                        }
                        BusMessage::Unsubscribe { subscription_id, ack } => {
                            let removed = self.remove_listener(subscription_id);
                            if let Err(_e) = ack.send(removed) {
                                warn!(subscription_id = subscription_id.as_u64(), "unsubscribe.ack_receiver_dropped");
                            }
                        }
                        BusMessage::Publish {
                            event_type,
                            event,
                            event_name,
                            ack,
                        } => {
                            self.dispatch(event_type, event, event_name).await;
                            if let Some(ack) = ack
                                && let Err(_e) = ack.send(())
                            {
                                warn!(event = event_name, "publish.ack_receiver_dropped");
                            }
                        }
                        BusMessage::Shutdown { ack } => {
                            self.rx.close();
                            self.drain_queued_messages().await;
                            let timed_out = self.drain_async_tasks().await;
                            let result = if timed_out {
                                Err(crate::error::EventBusError::ShutdownTimeout)
                            } else {
                                Ok(())
                            };
                            if let Err(_e) = ack.send(result) {
                                warn!("shutdown.ack_receiver_dropped");
                            }
                            break;
                        }
                    }

                    // Also reap any tasks that finished while we were handling
                    // the message above.
                    self.reap_finished_async_tasks();
                }

                // An in-flight async task completed while the channel was idle.
                Some(result) = self.async_tasks.join_next() => {
                    self.handle_join_result(result);
                }
            }
        }

        let _ = self.drain_async_tasks().await;
        debug!("event_bus_actor.stopped");
    }

    fn next_subscription_id(&mut self) -> SubscriptionId {
        let id = SubscriptionId(self.next_subscription_id);
        self.next_subscription_id = self
            .next_subscription_id
            .checked_add(1)
            .expect("subscription ID overflow: exceeded u64::MAX subscriptions");
        id
    }

    /// Remove a listener by its subscription ID.
    ///
    /// # Complexity
    ///
    /// This is **O(n)** in the number of listeners for the event type, where
    /// `n` is the length of the listener list.  The inner `Vec` is scanned
    /// linearly via `position()` and the element is removed with
    /// `swap_remove()` (O(1) removal once the index is found, but changes
    /// listener ordering).
    ///
    /// For typical workloads (tens to low hundreds of listeners per event
    /// type) this is fast.  If you have thousands of listeners per type and
    /// frequently unsubscribe, consider profiling.
    fn remove_listener(&mut self, subscription_id: SubscriptionId) -> bool {
        let Some(event_type) = self.listener_index.remove(&subscription_id) else {
            return false;
        };

        let mut remove_key = false;
        let mut removed = false;

        if let Some(list) = self.listeners.get_mut(&event_type) {
            let vec = Arc::make_mut(list);
            if let Some(index) = vec.iter().position(|listener| listener.id == subscription_id) {
                vec.swap_remove(index);
                removed = true;
            }
            remove_key = vec.is_empty();
        }

        if remove_key {
            self.listeners.remove(&event_type);
        }

        removed
    }

    async fn drain_queued_messages(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                BusMessage::Subscribe { ack, .. } => {
                    drop(ack);
                }
                BusMessage::Unsubscribe { subscription_id, ack } => {
                    let removed = self.remove_listener(subscription_id);
                    let _ = ack.send(removed);
                }
                BusMessage::Publish {
                    event_type,
                    event,
                    event_name,
                    ack,
                } => {
                    self.dispatch(event_type, event, event_name).await;
                    if let Some(ack) = ack
                        && let Err(_e) = ack.send(())
                    {
                        warn!(event = event_name, "publish.ack_receiver_dropped");
                    }
                }
                BusMessage::Shutdown { ack } => {
                    let _ = ack.send(Ok(()));
                }
            }

            self.reap_finished_async_tasks();
        }
    }

    /// Dispatch an event to all registered listeners for its type.
    ///
    /// # Allocation costs per publish
    ///
    /// Each call to `publish()` allocates `Arc::new(event)` once (on the
    /// caller side, before reaching this method).  During dispatch:
    ///
    /// - **Async listeners**: each invocation clones the `Arc<Event>` (cheap
    ///   ref-count bump) and allocates a `Box::pin(Future)` for the type-
    ///   erased handler future.  This boxing is inherent to the type-erasure
    ///   design and cannot be avoided without monomorphisation.
    /// - **Sync listeners**: receive `&dyn Any` — no per-listener allocation.
    /// - **Listener list snapshot**: the `Arc<Vec<Listener>>` is cloned (O(1)
    ///   ref-count) so dispatch can iterate without holding `&mut self`.
    ///   Mutations during dispatch trigger copy-on-write (`Arc::make_mut`).
    async fn dispatch(&mut self, event_type: TypeId, event: EventType, event_name: &'static str) {
        let Some(listeners) = self.listeners.get(&event_type).cloned() else {
            #[cfg(feature = "metrics")]
            counter!("eventbus.publish", "event" => event_name).increment(1);

            debug!(event = event_name, listeners = 0, "publish.dispatch");
            return;
        };

        #[cfg(feature = "metrics")]
        counter!("eventbus.publish", "event" => event_name).increment(1);

        let listeners_count = listeners.len();
        debug!(event = event_name, listeners = listeners_count, "publish.dispatch");

        let handler_timeout = self.handler_timeout;

        for listener in listeners.iter() {
            let listener_id = listener.id;
            let failure_policy = listener.failure_policy;

            match listener.handler {
                ErasedHandler::Async(ref handler) => {
                    let handler = Arc::clone(handler);
                    let event = Arc::clone(&event);
                    let span = tracing::trace_span!("eventbus.handler", event = event_name, mode = "async", listener_id = listener_id.as_u64());
                    let execution =
                        Self::execute_async_listener(handler, event, event_name, listener_id, failure_policy, handler_timeout).instrument(span);

                    debug!(event = event_name, listener_id = listener_id.as_u64(), "publish.async");
                    if let Some(ref sem) = self.async_semaphore {
                        let permit = Arc::clone(sem);
                        self.async_tasks.spawn(async move {
                            match permit.acquire().await {
                                Ok(_permit) => execution.await,
                                Err(_) => {
                                    warn!(event = event_name, listener_id = listener_id.as_u64(), "handler.semaphore_closed");
                                    None
                                }
                            }
                        });
                    } else {
                        self.async_tasks.spawn(execution);
                    }
                }
                ErasedHandler::Sync(ref handler) => {
                    let span = tracing::trace_span!("eventbus.handler", event = event_name, mode = "sync", listener_id = listener_id.as_u64());
                    let _enter = span.enter();

                    debug!(event = event_name, listener_id = listener_id.as_u64(), "publish.sync");

                    // Sync handlers execute exactly once — no retries.
                    // On failure, a dead letter is emitted (if enabled).
                    #[cfg(feature = "metrics")]
                    let _timer = TimerGuard::start("eventbus.handler.duration", event_name);

                    let result = Self::invoke_sync_handler(handler, &event);
                    if let Err(err) = result {
                        self.handle_listener_failure(ListenerFailure {
                            event_name,
                            subscription_id: listener_id,
                            attempts: 1,
                            error: err.to_string(),
                            dead_letter: failure_policy.dead_letter,
                        });
                    }
                }
            }
        }
    }

    /// Invoke a sync handler with `catch_unwind` for panic isolation.
    ///
    /// Sync handlers execute exactly once — no retries. On failure, the caller
    /// creates a [`ListenerFailure`] which flows through the dead-letter pipeline.
    ///
    /// # Safety rationale for `AssertUnwindSafe`
    ///
    /// The `catch_unwind` + `AssertUnwindSafe` combination is safe here because:
    /// - The handler closure captures only `&(dyn Any + Send + Sync)` (an
    ///   immutable reference) and the handler function pointer, neither of
    ///   which can be left in an inconsistent state by a panic.
    /// - No mutable actor state is accessed inside the `catch_unwind` block.
    /// - Panics are converted into `Err` results that flow through the normal
    ///   dead-letter pipeline.
    fn invoke_sync_handler(handler: &ErasedSyncHandler, event: &EventType) -> HandlerResult {
        let handler_ref = handler.as_ref();
        let event_ref: &(dyn Any + Send + Sync) = event.as_ref();
        let result = catch_unwind(AssertUnwindSafe(|| handler_ref(event_ref)));

        match result {
            Ok(r) => r,
            Err(panic_payload) => {
                let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "handler panicked".to_string()
                };
                Err(msg.into())
            }
        }
    }

    async fn execute_async_listener(
        handler: ErasedAsyncHandler,
        event: EventType,
        event_name: &'static str,
        subscription_id: SubscriptionId,
        failure_policy: FailurePolicy,
        handler_timeout: Option<Duration>,
    ) -> HandlerTaskOutcome {
        let mut retries_left = failure_policy.max_retries;
        let mut attempts = 0;

        loop {
            attempts += 1;

            #[cfg(feature = "metrics")]
            let _timer = TimerGuard::start("eventbus.handler.duration", event_name);

            let result = match handler_timeout {
                Some(timeout) => match tokio::time::timeout(timeout, handler(Arc::clone(&event))).await {
                    Ok(inner) => inner,
                    Err(_elapsed) => Err(format!("handler timed out after {timeout:?}").into()),
                },
                None => handler(Arc::clone(&event)).await,
            };

            match result {
                Ok(()) => return None,
                Err(err) => {
                    let error_message = err.to_string();
                    if retries_left == 0 {
                        return Some(ListenerFailure {
                            event_name,
                            subscription_id,
                            attempts,
                            error: error_message,
                            dead_letter: failure_policy.dead_letter,
                        });
                    }

                    retries_left -= 1;
                    warn!(
                        event = event_name,
                        listener_id = subscription_id.as_u64(),
                        attempts,
                        retries_left,
                        error = %error_message,
                        "handler.retry"
                    );

                    if let Some(delay) = failure_policy.retry_delay {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
    }

    /// Log the failure and record metrics.  Returns `Some(DeadLetter)` when
    /// the failure policy requests one and the event is not itself a dead
    /// letter (preventing recursion).
    fn log_failure(&self, failure: &ListenerFailure) -> Option<DeadLetter> {
        error!(
            event = failure.event_name,
            listener_id = failure.subscription_id.as_u64(),
            attempts = failure.attempts,
            error = %failure.error,
            "handler.failed"
        );

        #[cfg(feature = "metrics")]
        counter!("eventbus.handler.error", "event" => failure.event_name).increment(1);

        let dead_letter_type = std::any::type_name::<DeadLetter>();
        if failure.dead_letter && failure.event_name != dead_letter_type {
            Some(DeadLetter {
                event_name: failure.event_name,
                subscription_id: failure.subscription_id,
                attempts: failure.attempts,
                error: failure.error.clone(),
            })
        } else {
            None
        }
    }

    /// Handle a failure during normal (non-drain) operation by enqueuing the
    /// dead letter through the actor's own channel so it is processed in a
    /// future loop iteration.
    fn handle_listener_failure(&mut self, failure: ListenerFailure) {
        if let Some(dead_letter) = self.log_failure(&failure) {
            let dead_letter_type = std::any::type_name::<DeadLetter>();
            match self.tx.try_send(BusMessage::Publish {
                event_type: TypeId::of::<DeadLetter>(),
                event: Arc::new(dead_letter),
                event_name: dead_letter_type,
                ack: None,
            }) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    warn!(
                        event = failure.event_name,
                        listener_id = failure.subscription_id.as_u64(),
                        "dead_letter.drop.channel_full"
                    );

                    #[cfg(feature = "metrics")]
                    counter!("eventbus.dead_letter.drop", "reason" => "channel_full", "event" => failure.event_name).increment(1);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    warn!(
                        event = failure.event_name,
                        listener_id = failure.subscription_id.as_u64(),
                        "dead_letter.drop.actor_stopped"
                    );

                    #[cfg(feature = "metrics")]
                    counter!("eventbus.dead_letter.drop", "reason" => "actor_stopped", "event" => failure.event_name).increment(1);
                }
            }
        }
    }

    /// Handle a failure during drain/shutdown by dispatching the dead letter
    /// directly to registered listeners, bypassing the (possibly closed)
    /// channel.
    async fn handle_listener_failure_direct(&mut self, failure: ListenerFailure) {
        if let Some(dead_letter) = self.log_failure(&failure) {
            let dead_letter_type = std::any::type_name::<DeadLetter>();
            self.dispatch(TypeId::of::<DeadLetter>(), Arc::new(dead_letter), dead_letter_type).await;
        }
    }

    fn reap_finished_async_tasks(&mut self) {
        while let Some(result) = self.async_tasks.try_join_next() {
            self.handle_join_result(result);
        }
    }

    /// Process the outcome of a completed async handler task.
    ///
    /// Panics in handler tasks are converted into [`ListenerFailure`]s so they
    /// flow through the normal dead-letter pipeline instead of being silently
    /// dropped.
    fn handle_join_result(&mut self, result: Result<HandlerTaskOutcome, tokio::task::JoinError>) {
        match result {
            Ok(Some(failure)) => self.handle_listener_failure(failure),
            Ok(None) => {}
            Err(join_error) => {
                error!(error = %join_error, "handler.join_error");

                #[cfg(feature = "metrics")]
                counter!("eventbus.handler.join_error").increment(1);

                // Treat panics / cancellations as terminal failures eligible
                // for dead-lettering.
                let failure = ListenerFailure {
                    event_name: "unknown",
                    subscription_id: SubscriptionId(0),
                    attempts: 1,
                    error: join_error.to_string(),
                    dead_letter: true,
                };
                self.handle_listener_failure(failure);
            }
        }
    }

    /// Drain in-flight async tasks. Returns `true` if the shutdown timeout
    /// fired and remaining tasks were aborted.
    async fn drain_async_tasks(&mut self) -> bool {
        match self.shutdown_timeout {
            Some(timeout) => {
                let deadline = tokio::time::Instant::now() + timeout;
                loop {
                    match tokio::time::timeout_at(deadline, self.async_tasks.join_next()).await {
                        Ok(Some(result)) => self.handle_join_result_direct(result).await,
                        Ok(None) => return false, // all tasks done before deadline
                        Err(_elapsed) => {
                            let remaining = self.async_tasks.len();
                            warn!(remaining, "shutdown.timeout_reached, aborting remaining tasks");
                            self.async_tasks.abort_all();
                            // Drain the abort results
                            while self.async_tasks.join_next().await.is_some() {}
                            return true;
                        }
                    }
                }
            }
            None => {
                while let Some(result) = self.async_tasks.join_next().await {
                    self.handle_join_result_direct(result).await;
                }
                false
            }
        }
    }

    /// Like [`handle_join_result`] but dispatches dead letters directly during
    /// shutdown, bypassing the (possibly closed) channel.
    async fn handle_join_result_direct(&mut self, result: Result<HandlerTaskOutcome, tokio::task::JoinError>) {
        match result {
            Ok(Some(failure)) => self.handle_listener_failure_direct(failure).await,
            Ok(None) => {}
            Err(join_error) => {
                error!(error = %join_error, "handler.join_error");

                #[cfg(feature = "metrics")]
                counter!("eventbus.handler.join_error").increment(1);

                let failure = ListenerFailure {
                    event_name: "unknown",
                    subscription_id: SubscriptionId(0),
                    attempts: 1,
                    error: join_error.to_string(),
                    dead_letter: true,
                };
                self.handle_listener_failure_direct(failure).await;
            }
        }
    }
}
