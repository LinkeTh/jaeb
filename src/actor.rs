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

use crate::error::{EventBusError, HandlerResult};
use crate::middleware::MiddlewareDecision;
use crate::types::{BusConfig, BusStats, DeadLetter, Event, FailurePolicy, ListenerInfo, SubscriptionId};

#[cfg(feature = "metrics")]
use crate::metrics::TimerGuard;

pub(crate) type HandlerFuture = Pin<Box<dyn Future<Output = HandlerResult> + Send>>;
pub(crate) type EventType = Arc<dyn Any + Send + Sync>;

pub(crate) type TypedAsyncHandlerFn<E> = Arc<dyn Fn(Arc<E>) -> HandlerFuture + Send + Sync + 'static>;
pub(crate) type TypedSyncHandlerFn<E> = Arc<dyn Fn(&E) -> HandlerResult + Send + Sync + 'static>;

#[derive(Clone)]
pub(crate) enum TypedHandler<E> {
    Async(TypedAsyncHandlerFn<E>),
    Sync(TypedSyncHandlerFn<E>),
}

pub(crate) type ErasedAsyncMiddleware =
    Arc<dyn Fn(&'static str, EventType) -> Pin<Box<dyn Future<Output = MiddlewareDecision> + Send>> + Send + Sync>;
pub(crate) type ErasedSyncMiddleware = Arc<dyn Fn(&'static str, &(dyn Any + Send + Sync)) -> MiddlewareDecision + Send + Sync>;

#[derive(Clone)]
pub(crate) enum ErasedMiddleware {
    Async(ErasedAsyncMiddleware),
    Sync(ErasedSyncMiddleware),
}

pub(crate) type TypedAsyncMiddlewareFn<E> =
    Arc<dyn Fn(&'static str, Arc<E>) -> Pin<Box<dyn Future<Output = MiddlewareDecision> + Send>> + Send + Sync + 'static>;
pub(crate) type TypedSyncMiddlewareFn<E> = Arc<dyn Fn(&'static str, &E) -> MiddlewareDecision + Send + Sync + 'static>;

#[derive(Clone)]
pub(crate) enum TypedMiddlewareEntry<E> {
    Async(TypedAsyncMiddlewareFn<E>),
    Sync(TypedSyncMiddlewareFn<E>),
}

#[derive(Clone, Copy)]
pub(crate) struct ListenerMeta {
    pub id: SubscriptionId,
    pub failure_policy: FailurePolicy,
    pub name: Option<&'static str>,
    pub once: bool,
}

pub(crate) type RegisterFn = Box<dyn FnOnce(&mut ListenerRegistry, ListenerMeta) + Send>;
pub(crate) type RegisterTypedMiddlewareFn = Box<dyn FnOnce(&mut ListenerRegistry, SubscriptionId) + Send>;

#[derive(Clone)]
struct TypedListener<E> {
    id: SubscriptionId,
    handler: TypedHandler<E>,
    failure_policy: FailurePolicy,
    name: Option<&'static str>,
    once: bool,
}

#[derive(Clone)]
struct TypedMiddlewareSlot<E> {
    id: SubscriptionId,
    middleware: TypedMiddlewareEntry<E>,
}

#[derive(Default)]
struct TypedListenerList<E> {
    listeners: Vec<TypedListener<E>>,
    middlewares: Vec<TypedMiddlewareSlot<E>>,
}

#[derive(Debug)]
pub(crate) struct ListenerFailure {
    event_name: &'static str,
    subscription_id: SubscriptionId,
    attempts: usize,
    error: String,
    dead_letter: bool,
    event: EventType,
    listener_name: Option<&'static str>,
}

type HandlerTaskOutcome = Option<ListenerFailure>;

#[derive(Default)]
struct DispatchOutcome {
    failures: Vec<ListenerFailure>,
    removed_once_ids: Vec<SubscriptionId>,
}

trait AnyListenerList: Send + Sync {
    fn dispatch<'a>(
        &'a mut self,
        event: &'a EventType,
        event_name: &'static str,
        async_tasks: &'a mut JoinSet<HandlerTaskOutcome>,
        handler_timeout: Option<Duration>,
        async_semaphore: Option<&'a Arc<Semaphore>>,
    ) -> Pin<Box<dyn Future<Output = Result<DispatchOutcome, EventBusError>> + Send + 'a>>;

    fn remove_subscription(&mut self, id: SubscriptionId) -> bool;
    fn listener_count(&self) -> usize;
    fn listener_infos(&self) -> Vec<ListenerInfo>;
    fn is_empty(&self) -> bool;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<E> AnyListenerList for TypedListenerList<E>
where
    E: Event,
{
    fn dispatch<'a>(
        &'a mut self,
        event: &'a EventType,
        event_name: &'static str,
        async_tasks: &'a mut JoinSet<HandlerTaskOutcome>,
        handler_timeout: Option<Duration>,
        async_semaphore: Option<&'a Arc<Semaphore>>,
    ) -> Pin<Box<dyn Future<Output = Result<DispatchOutcome, EventBusError>> + Send + 'a>> {
        Box::pin(async move {
            let event = match Arc::clone(event).downcast::<E>() {
                Ok(event) => event,
                Err(_) => {
                    error!(expected = std::any::type_name::<E>(), event = event_name, "dispatch.downcast_failed");
                    return Ok(DispatchOutcome::default());
                }
            };

            if !self.middlewares.is_empty() {
                for slot in &self.middlewares {
                    let decision = match &slot.middleware {
                        TypedMiddlewareEntry::Async(mw) => mw(event_name, Arc::clone(&event)).await,
                        TypedMiddlewareEntry::Sync(mw) => mw(event_name, event.as_ref()),
                    };

                    if let MiddlewareDecision::Reject(reason) = decision {
                        debug!(event = event_name, reason = %reason, "publish.typed_middleware_rejected");
                        return Err(EventBusError::MiddlewareRejected(reason));
                    }
                }
            }

            let mut removed_once_ids = Vec::new();
            let mut retained_listeners = Vec::with_capacity(self.listeners.len());
            let listeners = std::mem::take(&mut self.listeners);

            let mut failures = Vec::new();
            for listener in listeners {
                let listener_id = listener.id;
                let failure_policy = listener.failure_policy;
                let listener_name = listener.name;

                match &listener.handler {
                    TypedHandler::Async(handler) => {
                        let handler = Arc::clone(handler);
                        let event = Arc::clone(&event);
                        let span = tracing::trace_span!(
                            "eventbus.handler",
                            event = event_name,
                            mode = "async",
                            listener_id = listener_id.as_u64(),
                            listener_name = listener_name.unwrap_or("unnamed"),
                        );

                        let execution = EventBusActor::execute_async_listener(
                            handler,
                            event,
                            event_name,
                            listener_id,
                            listener_name,
                            failure_policy,
                            handler_timeout,
                        )
                        .instrument(span);

                        debug!(event = event_name, listener_id = listener_id.as_u64(), "publish.async");

                        if let Some(semaphore) = async_semaphore {
                            let permit = Arc::clone(semaphore);
                            async_tasks.spawn(async move {
                                match permit.acquire().await {
                                    Ok(_permit) => execution.await,
                                    Err(_) => {
                                        warn!(event = event_name, listener_id = listener_id.as_u64(), "handler.semaphore_closed");
                                        None
                                    }
                                }
                            });
                        } else {
                            async_tasks.spawn(execution);
                        }
                    }
                    TypedHandler::Sync(handler) => {
                        let span = tracing::trace_span!(
                            "eventbus.handler",
                            event = event_name,
                            mode = "sync",
                            listener_id = listener_id.as_u64(),
                            listener_name = listener_name.unwrap_or("unnamed"),
                        );
                        let _enter = span.enter();

                        debug!(event = event_name, listener_id = listener_id.as_u64(), "publish.sync");

                        #[cfg(feature = "metrics")]
                        let _timer = TimerGuard::start("eventbus.handler.duration", event_name);

                        if let Err(err) = EventBusActor::invoke_sync_handler(handler, event.as_ref()) {
                            let failed_event: EventType = event.clone();
                            failures.push(ListenerFailure {
                                event_name,
                                subscription_id: listener_id,
                                attempts: 1,
                                error: err.to_string(),
                                dead_letter: failure_policy.dead_letter,
                                event: failed_event,
                                listener_name,
                            });
                        }
                    }
                }

                if listener.once {
                    removed_once_ids.push(listener_id);
                } else {
                    retained_listeners.push(listener);
                }
            }

            self.listeners = retained_listeners;

            Ok(DispatchOutcome { failures, removed_once_ids })
        })
    }

    fn remove_subscription(&mut self, id: SubscriptionId) -> bool {
        if let Some(index) = self.listeners.iter().position(|listener| listener.id == id) {
            self.listeners.remove(index);
            return true;
        }

        if let Some(index) = self.middlewares.iter().position(|slot| slot.id == id) {
            self.middlewares.remove(index);
            return true;
        }

        false
    }

    fn listener_count(&self) -> usize {
        self.listeners.len()
    }

    fn listener_infos(&self) -> Vec<ListenerInfo> {
        self.listeners
            .iter()
            .map(|listener| ListenerInfo {
                subscription_id: listener.id,
                name: listener.name,
            })
            .collect()
    }

    fn is_empty(&self) -> bool {
        self.listeners.is_empty() && self.middlewares.is_empty()
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Default)]
pub(crate) struct ListenerRegistry {
    map: HashMap<TypeId, Box<dyn AnyListenerList>>,
    index: HashMap<SubscriptionId, TypeId>,
    type_names: HashMap<TypeId, &'static str>,
}

impl ListenerRegistry {
    pub(crate) fn register_type_name(&mut self, event_type: TypeId, event_type_name: &'static str) {
        self.type_names.entry(event_type).or_insert(event_type_name);
    }

    fn ensure_list_mut<E: Event>(&mut self) -> &mut TypedListenerList<E> {
        let entry = self.map.entry(TypeId::of::<E>()).or_insert_with(|| {
            Box::new(TypedListenerList::<E> {
                listeners: Vec::new(),
                middlewares: Vec::new(),
            })
        });

        entry
            .as_any_mut()
            .downcast_mut::<TypedListenerList<E>>()
            .expect("listener registry type mismatch")
    }

    pub(crate) fn add_typed_listener<E: Event>(&mut self, meta: ListenerMeta, handler: TypedHandler<E>) {
        self.register_type_name(TypeId::of::<E>(), std::any::type_name::<E>());
        let list = self.ensure_list_mut::<E>();
        list.listeners.push(TypedListener {
            id: meta.id,
            handler,
            failure_policy: meta.failure_policy,
            name: meta.name,
            once: meta.once,
        });
        self.index.insert(meta.id, TypeId::of::<E>());
    }

    pub(crate) fn add_typed_middleware<E: Event>(&mut self, subscription_id: SubscriptionId, middleware: TypedMiddlewareEntry<E>) {
        self.register_type_name(TypeId::of::<E>(), std::any::type_name::<E>());
        let list = self.ensure_list_mut::<E>();
        list.middlewares.push(TypedMiddlewareSlot {
            id: subscription_id,
            middleware,
        });
        self.index.insert(subscription_id, TypeId::of::<E>());
    }

    pub(crate) fn listener_count_for(&self, event_type: TypeId) -> usize {
        self.map.get(&event_type).map_or(0, |list| list.listener_count())
    }

    pub(crate) async fn dispatch(
        &mut self,
        event_type: TypeId,
        event: &EventType,
        event_name: &'static str,
        async_tasks: &mut JoinSet<HandlerTaskOutcome>,
        handler_timeout: Option<Duration>,
        async_semaphore: Option<&Arc<Semaphore>>,
    ) -> Result<Vec<ListenerFailure>, EventBusError> {
        let (dispatch_outcome, remove_key) = {
            let Some(list) = self.map.get_mut(&event_type) else {
                return Ok(Vec::new());
            };

            let outcome = list.dispatch(event, event_name, async_tasks, handler_timeout, async_semaphore).await?;
            let remove_key = list.is_empty();
            (outcome, remove_key)
        };

        for removed_once_id in dispatch_outcome.removed_once_ids {
            self.index.remove(&removed_once_id);
        }

        if remove_key {
            self.map.remove(&event_type);
            self.type_names.remove(&event_type);
        }

        Ok(dispatch_outcome.failures)
    }

    pub(crate) fn remove_subscription(&mut self, subscription_id: SubscriptionId) -> bool {
        let Some(event_type) = self.index.remove(&subscription_id) else {
            return false;
        };

        let mut remove_key = false;
        let mut removed = false;

        if let Some(list) = self.map.get_mut(&event_type) {
            removed = list.remove_subscription(subscription_id);
            remove_key = list.is_empty();
        }

        if remove_key {
            self.map.remove(&event_type);
            self.type_names.remove(&event_type);
        }

        removed
    }

    pub(crate) fn collect_listener_stats(&self) -> (usize, HashMap<&'static str, Vec<ListenerInfo>>, Vec<&'static str>) {
        let mut total_subscriptions = 0;
        let mut subscriptions_by_event = HashMap::new();
        let mut registered_event_types = Vec::new();

        for (type_id, list) in &self.map {
            let infos = list.listener_infos();
            if infos.is_empty() {
                continue;
            }

            let event_name = self.type_names.get(type_id).copied().unwrap_or("unknown");
            total_subscriptions += infos.len();
            registered_event_types.push(event_name);
            subscriptions_by_event.insert(event_name, infos);
        }

        registered_event_types.sort_unstable();

        (total_subscriptions, subscriptions_by_event, registered_event_types)
    }
}

pub(crate) enum BusMessage {
    Subscribe {
        event_type: TypeId,
        event_type_name: &'static str,
        register: RegisterFn,
        failure_policy: FailurePolicy,
        listener_name: Option<&'static str>,
        once: bool,
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
        ack: Option<oneshot::Sender<Result<(), EventBusError>>>,
    },
    AddMiddleware {
        middleware: ErasedMiddleware,
        ack: oneshot::Sender<SubscriptionId>,
    },
    AddTypedMiddleware {
        event_type: TypeId,
        event_type_name: &'static str,
        register: RegisterTypedMiddlewareFn,
        ack: oneshot::Sender<SubscriptionId>,
    },
    Stats {
        ack: oneshot::Sender<BusStats>,
    },
    Shutdown {
        ack: oneshot::Sender<Result<(), EventBusError>>,
    },
}

impl BusMessage {
    pub(crate) fn operation_name(&self) -> &'static str {
        match self {
            Self::Subscribe { .. } => "subscribe",
            Self::Unsubscribe { .. } => "unsubscribe",
            Self::Publish { .. } => "publish",
            Self::AddMiddleware { .. } => "add_middleware",
            Self::AddTypedMiddleware { .. } => "add_typed_middleware",
            Self::Stats { .. } => "stats",
            Self::Shutdown { .. } => "shutdown",
        }
    }
}

pub(crate) struct EventBusActor {
    tx: mpsc::Sender<BusMessage>,
    rx: mpsc::Receiver<BusMessage>,
    registry: ListenerRegistry,
    next_subscription_id: u64,
    async_tasks: JoinSet<HandlerTaskOutcome>,
    handler_timeout: Option<Duration>,
    async_semaphore: Option<Arc<Semaphore>>,
    shutdown_timeout: Option<Duration>,
    buffer_size: usize,
    middlewares: Vec<(SubscriptionId, ErasedMiddleware)>,
}

impl EventBusActor {
    pub fn new(tx: mpsc::Sender<BusMessage>, rx: mpsc::Receiver<BusMessage>, config: &BusConfig) -> Self {
        let async_semaphore = config.max_concurrent_async.map(|n| Arc::new(Semaphore::new(n)));
        trace!("event_bus_actor.init {:?}", &config);

        Self {
            tx,
            rx,
            registry: ListenerRegistry::default(),
            next_subscription_id: 1,
            async_tasks: JoinSet::new(),
            handler_timeout: config.handler_timeout,
            async_semaphore,
            shutdown_timeout: config.shutdown_timeout,
            buffer_size: config.buffer_size,
            middlewares: Vec::new(),
        }
    }

    pub async fn run(mut self) {
        trace!("event_bus_actor.run");

        loop {
            tokio::select! {
                biased;

                msg = self.rx.recv() => {
                    let Some(msg) = msg else {
                        break;
                    };

                    self.reap_finished_async_tasks();

                    match msg {
                        BusMessage::Subscribe {
                            event_type,
                            event_type_name,
                            register,
                            failure_policy,
                            listener_name,
                            once,
                            ack,
                        } => {
                            let id = self.next_subscription_id();
                            self.registry.register_type_name(event_type, event_type_name);
                            register(
                                &mut self.registry,
                                ListenerMeta {
                                    id,
                                    failure_policy,
                                    name: listener_name,
                                    once,
                                },
                            );

                            if let Err(_e) = ack.send(id) {
                                warn!("subscribe.ack_receiver_dropped");
                            }
                        }
                        BusMessage::Unsubscribe { subscription_id, ack } => {
                            let removed = self.registry.remove_subscription(subscription_id) || self.remove_middleware(subscription_id);
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
                            let result = self.dispatch(event_type, event, event_name).await;
                            if let Some(ack) = ack
                                && let Err(_e) = ack.send(result)
                            {
                                warn!(event = event_name, "publish.ack_receiver_dropped");
                            }
                        }
                        BusMessage::AddMiddleware { middleware, ack } => {
                            let id = self.next_subscription_id();
                            self.middlewares.push((id, middleware));
                            if let Err(_e) = ack.send(id) {
                                warn!("add_middleware.ack_receiver_dropped");
                            }
                        }
                        BusMessage::AddTypedMiddleware {
                            event_type,
                            event_type_name,
                            register,
                            ack,
                        } => {
                            let id = self.next_subscription_id();
                            self.registry.register_type_name(event_type, event_type_name);
                            register(&mut self.registry, id);
                            if let Err(_e) = ack.send(id) {
                                warn!("add_typed_middleware.ack_receiver_dropped");
                            }
                        }
                        BusMessage::Stats { ack } => {
                            let stats = self.collect_stats(false);
                            if let Err(_e) = ack.send(stats) {
                                warn!("stats.ack_receiver_dropped");
                            }
                        }
                        BusMessage::Shutdown { ack } => {
                            self.rx.close();
                            self.drain_queued_messages().await;
                            let timed_out = self.drain_async_tasks().await;
                            let result = if timed_out {
                                Err(EventBusError::ShutdownTimeout)
                            } else {
                                Ok(())
                            };

                            if let Err(_e) = ack.send(result) {
                                warn!("shutdown.ack_receiver_dropped");
                            }

                            break;
                        }
                    }

                    self.reap_finished_async_tasks();
                }

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

    fn remove_middleware(&mut self, subscription_id: SubscriptionId) -> bool {
        let len_before = self.middlewares.len();
        self.middlewares.retain(|(id, _)| *id != subscription_id);
        self.middlewares.len() < len_before
    }

    fn collect_stats(&self, shutdown_called: bool) -> BusStats {
        let (total_subscriptions, subscriptions_by_event, registered_event_types) = self.registry.collect_listener_stats();

        BusStats {
            total_subscriptions,
            subscriptions_by_event,
            registered_event_types,
            queue_capacity: self.buffer_size,
            in_flight_async: self.async_tasks.len(),
            shutdown_called,
        }
    }

    async fn drain_queued_messages(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                BusMessage::Subscribe { ack, .. } => {
                    drop(ack);
                }
                BusMessage::Unsubscribe { subscription_id, ack } => {
                    let removed = self.registry.remove_subscription(subscription_id) || self.remove_middleware(subscription_id);
                    let _ = ack.send(removed);
                }
                BusMessage::Publish {
                    event_type,
                    event,
                    event_name,
                    ack,
                } => {
                    let result = self.dispatch(event_type, event, event_name).await;
                    if let Some(ack) = ack
                        && let Err(_e) = ack.send(result)
                    {
                        warn!(event = event_name, "publish.ack_receiver_dropped");
                    }
                }
                BusMessage::AddMiddleware { ack, .. } => {
                    drop(ack);
                }
                BusMessage::AddTypedMiddleware { ack, .. } => {
                    drop(ack);
                }
                BusMessage::Shutdown { ack } => {
                    let _ = ack.send(Ok(()));
                }
                BusMessage::Stats { ack } => {
                    let stats = self.collect_stats(true);
                    let _ = ack.send(stats);
                }
            }

            self.reap_finished_async_tasks();
        }
    }

    async fn dispatch(&mut self, event_type: TypeId, event: EventType, event_name: &'static str) -> Result<(), EventBusError> {
        if !self.middlewares.is_empty() {
            for (_id, mw) in &self.middlewares {
                let decision = match mw {
                    ErasedMiddleware::Async(f) => f(event_name, Arc::clone(&event)).await,
                    ErasedMiddleware::Sync(f) => f(event_name, event.as_ref()),
                };

                if let MiddlewareDecision::Reject(reason) = decision {
                    debug!(event = event_name, reason = %reason, "publish.middleware_rejected");
                    return Err(EventBusError::MiddlewareRejected(reason));
                }
            }
        }

        #[cfg(feature = "metrics")]
        counter!("eventbus.publish", "event" => event_name).increment(1);

        let listeners_count = self.registry.listener_count_for(event_type);
        debug!(event = event_name, listeners = listeners_count, "publish.dispatch");

        let failures = self
            .registry
            .dispatch(
                event_type,
                &event,
                event_name,
                &mut self.async_tasks,
                self.handler_timeout,
                self.async_semaphore.as_ref(),
            )
            .await?;

        for failure in failures {
            self.handle_listener_failure(failure);
        }

        Ok(())
    }

    fn invoke_sync_handler<E: Event>(handler: &TypedSyncHandlerFn<E>, event: &E) -> HandlerResult {
        let handler_ref = handler.as_ref();
        let result = catch_unwind(AssertUnwindSafe(|| handler_ref(event)));

        result.unwrap_or_else(|panic_payload| {
            let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "handler panicked".to_string()
            };
            Err(msg.into())
        })
    }

    async fn execute_async_listener<E: Event>(
        handler: TypedAsyncHandlerFn<E>,
        event: Arc<E>,
        event_name: &'static str,
        subscription_id: SubscriptionId,
        listener_name: Option<&'static str>,
        failure_policy: FailurePolicy,
        handler_timeout: Option<Duration>,
    ) -> HandlerTaskOutcome {
        let mut retries_left = failure_policy.max_retries;
        let mut attempts = 0;

        loop {
            attempts += 1;

            #[cfg(feature = "metrics")]
            let _timer = TimerGuard::start("eventbus.handler.duration", event_name);

            let result = Self::run_async_attempt(handler.clone(), Arc::clone(&event), handler_timeout).await;

            match result {
                Ok(()) => return None,
                Err(err) => {
                    let error_message = err.to_string();

                    if retries_left == 0 {
                        let failed_event: EventType = event.clone();
                        return Some(ListenerFailure {
                            event_name,
                            subscription_id,
                            attempts,
                            error: error_message,
                            dead_letter: failure_policy.dead_letter,
                            event: failed_event,
                            listener_name,
                        });
                    }

                    retries_left -= 1;
                    let retry_attempt = failure_policy.max_retries - retries_left - 1;
                    warn!(
                        event = event_name,
                        listener_id = subscription_id.as_u64(),
                        attempts,
                        retries_left,
                        error = %error_message,
                        "handler.retry"
                    );

                    if let Some(strategy) = failure_policy.retry_strategy {
                        let delay = strategy.delay_for_attempt(retry_attempt);
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
    }

    async fn run_async_attempt<E: Event>(handler: TypedAsyncHandlerFn<E>, event: Arc<E>, handler_timeout: Option<Duration>) -> HandlerResult {
        match handler_timeout {
            Some(timeout) => {
                let mut join = tokio::spawn(handler(event));
                match tokio::time::timeout(timeout, &mut join).await {
                    Ok(Ok(inner)) => inner,
                    Ok(Err(join_error)) => Err(format!("handler task failed: {join_error}").into()),
                    Err(_elapsed) => {
                        join.abort();
                        let _ = join.await;
                        Err(format!("handler timed out after {timeout:?}").into())
                    }
                }
            }
            None => match tokio::spawn(handler(event)).await {
                Ok(inner) => inner,
                Err(join_error) => Err(format!("handler task failed: {join_error}").into()),
            },
        }
    }

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
                event: failure.event.clone(),
                failed_at: std::time::SystemTime::now(),
                listener_name: failure.listener_name,
            })
        } else {
            None
        }
    }

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

    async fn handle_listener_failure_direct(&mut self, failure: ListenerFailure) {
        if let Some(dead_letter) = self.log_failure(&failure) {
            let dead_letter_type = std::any::type_name::<DeadLetter>();
            let _ = self.dispatch(TypeId::of::<DeadLetter>(), Arc::new(dead_letter), dead_letter_type).await;
        }
    }

    fn reap_finished_async_tasks(&mut self) {
        while let Some(result) = self.async_tasks.try_join_next() {
            self.handle_join_result(result);
        }
    }

    fn handle_join_result(&mut self, result: Result<HandlerTaskOutcome, tokio::task::JoinError>) {
        match result {
            Ok(Some(failure)) => self.handle_listener_failure(failure),
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
                    event: Arc::new(()),
                    listener_name: None,
                };
                self.handle_listener_failure(failure);
            }
        }
    }

    async fn drain_async_tasks(&mut self) -> bool {
        match self.shutdown_timeout {
            Some(timeout) => {
                let deadline = tokio::time::Instant::now() + timeout;
                loop {
                    match tokio::time::timeout_at(deadline, self.async_tasks.join_next()).await {
                        Ok(Some(result)) => self.handle_join_result_direct(result).await,
                        Ok(None) => return false,
                        Err(_elapsed) => {
                            let remaining = self.async_tasks.len();
                            warn!(remaining, "shutdown.timeout_reached, aborting remaining tasks");
                            self.async_tasks.abort_all();
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
                    event: Arc::new(()),
                    listener_name: None,
                };
                self.handle_listener_failure_direct(failure).await;
            }
        }
    }
}
