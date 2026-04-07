use std::{
    any::{Any, TypeId},
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::Arc,
};

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, trace, warn};

#[cfg(feature = "metrics")]
use metrics::counter;

use crate::error::HandlerResult;
use crate::types::{DeadLetter, FailurePolicy, SubscriptionId};

#[cfg(feature = "metrics")]
use crate::metrics::TimerGuard;

pub(crate) type HandlerFuture = Pin<Box<dyn Future<Output = HandlerResult> + Send>>;
pub(crate) type ArcEvent = Arc<dyn Any + Send + Sync>;
pub(crate) type ErasedHandler = Arc<dyn Fn(ArcEvent) -> HandlerFuture + Send + Sync + 'static>;

#[derive(Clone, Copy)]
pub(crate) enum ListenerMode {
    Async,
    Sync,
}

#[derive(Clone)]
pub(crate) struct Listener {
    pub id: SubscriptionId,
    pub handler: ErasedHandler,
    pub mode: ListenerMode,
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
type EventType = Box<dyn Any + Send + Sync>;

pub(crate) enum BusMessage {
    Subscribe {
        event_type: TypeId,
        handler: ErasedHandler,
        mode: ListenerMode,
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
        ack: oneshot::Sender<()>,
    },
}

pub(crate) struct EventBusActor {
    tx: mpsc::Sender<BusMessage>,
    rx: mpsc::Receiver<BusMessage>,
    listeners: HashMap<TypeId, Vec<Listener>>,
    listener_index: HashMap<SubscriptionId, TypeId>,
    next_subscription_id: u64,
    async_tasks: JoinSet<HandlerTaskOutcome>,
}

impl EventBusActor {
    pub fn new(tx: mpsc::Sender<BusMessage>, rx: mpsc::Receiver<BusMessage>) -> Self {
        Self {
            tx,
            rx,
            listeners: HashMap::new(),
            listener_index: HashMap::new(),
            next_subscription_id: 1,
            async_tasks: JoinSet::new(),
        }
    }

    pub async fn run(mut self) {
        trace!("event_bus_actor.run");
        while let Some(msg) = self.rx.recv().await {
            match msg {
                BusMessage::Subscribe {
                    event_type,
                    handler,
                    mode,
                    failure_policy,
                    ack,
                } => {
                    let id = self.next_subscription_id();
                    let list = self.listeners.entry(event_type).or_default();
                    list.push(Listener {
                        id,
                        handler,
                        mode,
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
                    self.drain_async_tasks().await;
                    if let Err(_e) = ack.send(()) {
                        warn!("shutdown.ack_receiver_dropped");
                    }
                    break;
                }
            }

            self.reap_finished_async_tasks();
        }

        self.drain_async_tasks().await;
        debug!("event_bus_actor.stopped");
    }

    fn next_subscription_id(&mut self) -> SubscriptionId {
        let id = SubscriptionId(self.next_subscription_id);
        self.next_subscription_id = self.next_subscription_id.saturating_add(1);
        id
    }

    fn remove_listener(&mut self, subscription_id: SubscriptionId) -> bool {
        let Some(event_type) = self.listener_index.remove(&subscription_id) else {
            return false;
        };

        let mut remove_key = false;
        let mut removed = false;

        if let Some(list) = self.listeners.get_mut(&event_type) {
            if let Some(index) = list.iter().position(|listener| listener.id == subscription_id) {
                list.swap_remove(index);
                removed = true;
            }
            remove_key = list.is_empty();
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
                    let _ = ack.send(());
                }
            }

            self.reap_finished_async_tasks();
        }
    }

    async fn dispatch(&mut self, event_type: TypeId, event: EventType, event_name: &'static str) {
        let listeners = self.listeners.get(&event_type).cloned().unwrap_or_default();
        let event: ArcEvent = Arc::from(event);

        #[cfg(feature = "metrics")]
        counter!("eventbus.publish", "event" => event_name).increment(1);

        let listeners_count = listeners.len();
        debug!(event = event_name, listeners = listeners_count, "publish.dispatch");

        let mut sync_tasks = Vec::new();
        for listener in listeners {
            let handler = Arc::clone(&listener.handler);
            let event = Arc::clone(&event);
            let listener_id = listener.id;
            let failure_policy = listener.failure_policy;
            let span = match listener.mode {
                ListenerMode::Async => {
                    tracing::trace_span!("eventbus.handler", event = event_name, mode = "async", listener_id = listener_id.as_u64())
                }
                ListenerMode::Sync => tracing::trace_span!("eventbus.handler", event = event_name, mode = "sync", listener_id = listener_id.as_u64()),
            };

            let execution = Self::execute_listener(handler, event, event_name, listener_id, failure_policy).instrument(span);

            match listener.mode {
                ListenerMode::Async => {
                    debug!(event = event_name, listener_id = listener_id.as_u64(), "publish.async");
                    self.async_tasks.spawn(execution);
                }
                ListenerMode::Sync => {
                    debug!(event = event_name, listener_id = listener_id.as_u64(), "publish.sync");
                    sync_tasks.push(tokio::spawn(execution));
                }
            }
        }

        for task in sync_tasks {
            match task.await {
                Ok(Some(failure)) => self.handle_listener_failure(failure),
                Ok(None) => {}
                Err(join_error) => self.log_join_error(event_name, join_error),
            }
        }
    }

    async fn execute_listener(
        handler: ErasedHandler,
        event: ArcEvent,
        event_name: &'static str,
        subscription_id: SubscriptionId,
        failure_policy: FailurePolicy,
    ) -> HandlerTaskOutcome {
        let mut retries_left = failure_policy.max_retries;
        let mut attempts = 0;

        loop {
            attempts += 1;

            #[cfg(feature = "metrics")]
            let _timer = TimerGuard::start("eventbus.handler.duration", event_name);

            match handler(Arc::clone(&event)).await {
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
                event: Box::new(dead_letter),
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
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    warn!(
                        event = failure.event_name,
                        listener_id = failure.subscription_id.as_u64(),
                        "dead_letter.drop.actor_stopped"
                    );
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
            self.dispatch(TypeId::of::<DeadLetter>(), Box::new(dead_letter), dead_letter_type).await;
        }
    }

    fn reap_finished_async_tasks(&mut self) {
        while let Some(result) = self.async_tasks.try_join_next() {
            match result {
                Ok(Some(failure)) => self.handle_listener_failure(failure),
                Ok(None) => {}
                Err(join_error) => self.log_join_error("unknown", join_error),
            }
        }
    }

    async fn drain_async_tasks(&mut self) {
        while let Some(result) = self.async_tasks.join_next().await {
            match result {
                Ok(Some(failure)) => self.handle_listener_failure_direct(failure).await,
                Ok(None) => {}
                Err(join_error) => self.log_join_error("unknown", join_error),
            }
        }
    }

    fn log_join_error(&self, event_name: &'static str, join_error: tokio::task::JoinError) {
        error!(event = event_name, error = %join_error, "handler.join_error");

        #[cfg(feature = "metrics")]
        counter!("eventbus.handler.join_error", "event" => event_name).increment(1);
    }
}
