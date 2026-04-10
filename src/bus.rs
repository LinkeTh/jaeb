use std::any::{Any, TypeId};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use arc_swap::ArcSwap;
use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};
use tokio::task::JoinHandle;
use tracing::{error, trace, warn};

use crate::error::{ConfigError, EventBusError};
use crate::handler::{IntoHandler, RegisteredHandler, SyncEventHandler};
use crate::middleware::{Middleware, SyncMiddleware, TypedMiddleware, TypedSyncMiddleware};
use crate::registry::{
    dead_letter_from_failure, dispatch_with_snapshot, AsyncTaskTracker, ControlNotification, DispatchContext, ErasedMiddleware, MutableRegistry,
    RegistrySnapshot, TypedMiddlewareEntry, TypedMiddlewareSlot,
};
use crate::subscription::Subscription;
use crate::types::{BusConfig, BusStats, DeadLetter, Event, FailurePolicy, IntoFailurePolicy, NoRetryPolicy, SubscriptionId};

pub struct EventBusBuilder {
    config: BusConfig,
}

impl std::fmt::Debug for EventBusBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBusBuilder").field("config", &self.config).finish()
    }
}

impl EventBusBuilder {
    fn new() -> Self {
        Self {
            config: BusConfig::default(),
        }
    }

    pub fn buffer_size(mut self, size: usize) -> Self {
        self.config.buffer_size = size;
        self
    }

    pub fn handler_timeout(mut self, timeout: Duration) -> Self {
        self.config.handler_timeout = Some(timeout);
        self
    }

    pub fn max_concurrent_async(mut self, max: usize) -> Self {
        self.config.max_concurrent_async = Some(max);
        self
    }

    pub fn default_failure_policy(mut self, policy: FailurePolicy) -> Self {
        self.config.default_failure_policy = policy;
        self
    }

    pub fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.config.shutdown_timeout = Some(timeout);
        self
    }

    pub fn build(self) -> Result<EventBus, EventBusError> {
        if self.config.buffer_size == 0 {
            return Err(ConfigError::ZeroBufferSize.into());
        }
        if self.config.max_concurrent_async == Some(0) {
            return Err(ConfigError::ZeroConcurrency.into());
        }
        Ok(EventBus::from_config(self.config))
    }
}

struct Inner {
    snapshot: ArcSwap<RegistrySnapshot>,
    registry: Mutex<MutableRegistry>,
    default_failure_policy: FailurePolicy,
    tracker: Arc<AsyncTaskTracker>,
    publish_permits: Arc<Semaphore>,
    shutdown_timeout: Option<Duration>,
    handler_timeout: Option<Duration>,
    buffer_size: usize,
    shutdown_called: AtomicBool,
    next_subscription_id: AtomicU64,
    notify_tx: mpsc::UnboundedSender<ControlNotification>,
    control_handle: StdMutex<Option<JoinHandle<()>>>,
}

impl Inner {
    fn full_dispatch_context(&self) -> DispatchContext {
        DispatchContext {
            tracker: Arc::clone(&self.tracker),
            notify_tx: self.notify_tx.clone(),
            handler_timeout: self.handler_timeout,
            spawn_async_handlers: true,
        }
    }

    fn sync_only_dispatch_context(&self) -> DispatchContext {
        DispatchContext {
            tracker: Arc::clone(&self.tracker),
            notify_tx: self.notify_tx.clone(),
            handler_timeout: self.handler_timeout,
            spawn_async_handlers: false,
        }
    }
}

#[derive(Clone)]
pub struct EventBus {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("default_failure_policy", &self.inner.default_failure_policy)
            .field("shutdown_called", &self.inner.shutdown_called.load(Ordering::Relaxed))
            .finish()
    }
}

impl EventBus {
    pub fn new(buffer: usize) -> Result<Self, EventBusError> {
        Self::builder().buffer_size(buffer).build()
    }

    pub fn builder() -> EventBusBuilder {
        EventBusBuilder::new()
    }

    fn from_config(config: BusConfig) -> Self {
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let tracker = Arc::new(AsyncTaskTracker::default());
        let inner = Arc::new(Inner {
            snapshot: ArcSwap::from_pointee(RegistrySnapshot::default()),
            registry: Mutex::new(MutableRegistry::new(config.max_concurrent_async)),
            default_failure_policy: config.default_failure_policy,
            tracker,
            publish_permits: Arc::new(Semaphore::new(config.buffer_size)),
            shutdown_timeout: config.shutdown_timeout,
            handler_timeout: config.handler_timeout,
            buffer_size: config.buffer_size,
            shutdown_called: AtomicBool::new(false),
            next_subscription_id: AtomicU64::new(1),
            notify_tx,
            control_handle: StdMutex::new(None),
        });

        let control_handle = tokio::spawn(control_loop(Arc::downgrade(&inner), notify_rx));
        *inner.control_handle.lock().expect("control_handle lock poisoned") = Some(control_handle);

        Self { inner }
    }

    fn next_subscription_id(&self) -> SubscriptionId {
        let id = self
            .inner
            .next_subscription_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| current.checked_add(1));
        match id {
            Ok(id) => SubscriptionId(id),
            Err(_) => panic!("subscription ID overflow: exceeded u64::MAX subscriptions"),
        }
    }

    async fn refresh_snapshot_locked(&self, registry: &MutableRegistry) {
        self.inner.snapshot.store(Arc::new(registry.snapshot()));
    }

    pub(crate) fn try_unsubscribe_best_effort(&self, subscription_id: SubscriptionId) -> bool {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return false;
        }
        if let Ok(mut registry) = self.inner.registry.try_lock()
            && registry.remove_subscription(subscription_id)
        {
            self.inner.snapshot.store(Arc::new(registry.snapshot()));
            return true;
        }
        false
    }

    pub async fn subscribe<E, H, M>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        let registered = handler.into_handler();
        let mut policy = self.inner.default_failure_policy;
        if registered.is_sync {
            policy.max_retries = 0;
            policy.retry_strategy = None;
        }
        self.subscribe_internal::<E>(registered, policy, false).await
    }

    pub async fn subscribe_with_policy<E, H, M>(&self, handler: H, policy: impl IntoFailurePolicy<M>) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        let registered = handler.into_handler();
        self.subscribe_internal::<E>(registered, policy.into_failure_policy(), false).await
    }

    async fn subscribe_internal<E: Event>(
        &self,
        registered: RegisteredHandler,
        failure_policy: FailurePolicy,
        once: bool,
    ) -> Result<Subscription, EventBusError> {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }

        trace!("event_bus.subscribe {:?}", &registered.name);
        let id = self.next_subscription_id();
        let listener = (registered.register)(id, failure_policy, once);

        let mut registry = self.inner.registry.lock().await;
        registry.add_listener(TypeId::of::<E>(), std::any::type_name::<E>(), listener);
        self.refresh_snapshot_locked(&registry).await;

        Ok(Subscription::new(id, self.clone()))
    }

    pub async fn subscribe_dead_letters<H>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        H: SyncEventHandler<DeadLetter>,
    {
        let policy = NoRetryPolicy::default().with_dead_letter(false);
        self.subscribe_with_policy::<DeadLetter, H, crate::handler::SyncMode>(handler, policy)
            .await
    }

    pub async fn subscribe_once<E, H, M>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        let policy = NoRetryPolicy {
            dead_letter: self.inner.default_failure_policy.dead_letter,
        };
        self.subscribe_once_with_policy(handler, policy).await
    }

    pub async fn subscribe_once_with_policy<E, H, M>(&self, handler: H, failure_policy: NoRetryPolicy) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        let registered = handler.into_handler();
        self.subscribe_internal::<E>(registered, failure_policy.into(), true).await
    }

    pub async fn add_middleware<M: Middleware>(&self, middleware: M) -> Result<Subscription, EventBusError> {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }
        let mw = Arc::new(middleware);
        let erased = ErasedMiddleware::Async(Arc::new(move |event_name: &'static str, event| {
            let mw = Arc::clone(&mw);
            Box::pin(async move { mw.process(event_name, event.as_ref()).await }) as Pin<Box<dyn Future<Output = _> + Send>>
        }));

        let id = self.next_subscription_id();
        let mut registry = self.inner.registry.lock().await;
        registry.add_global_middleware(id, erased);
        self.refresh_snapshot_locked(&registry).await;
        Ok(Subscription::new(id, self.clone()))
    }

    pub async fn add_sync_middleware<M: SyncMiddleware>(&self, middleware: M) -> Result<Subscription, EventBusError> {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }
        let mw = Arc::new(middleware);
        let erased = ErasedMiddleware::Sync(Arc::new(move |event_name: &'static str, event: &(dyn Any + Send + Sync)| {
            mw.process(event_name, event)
        }));

        let id = self.next_subscription_id();
        let mut registry = self.inner.registry.lock().await;
        registry.add_global_middleware(id, erased);
        self.refresh_snapshot_locked(&registry).await;
        Ok(Subscription::new(id, self.clone()))
    }

    pub async fn add_typed_middleware<E, M>(&self, middleware: M) -> Result<Subscription, EventBusError>
    where
        E: Event,
        M: TypedMiddleware<E>,
    {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }
        let mw = Arc::new(middleware);
        let slot = TypedMiddlewareSlot {
            id: self.next_subscription_id(),
            middleware: TypedMiddlewareEntry::Async(Arc::new(move |event_name, event| {
                let mw = Arc::clone(&mw);
                let event = event.downcast::<E>();
                Box::pin(async move {
                    match event {
                        Ok(event) => mw.process(event_name, event.as_ref()).await,
                        Err(_) => crate::middleware::MiddlewareDecision::Reject("event type mismatch".to_string()),
                    }
                })
            })),
        };

        let mut registry = self.inner.registry.lock().await;
        registry.add_typed_middleware(TypeId::of::<E>(), std::any::type_name::<E>(), slot.clone());
        self.refresh_snapshot_locked(&registry).await;
        Ok(Subscription::new(slot.id, self.clone()))
    }

    pub async fn add_typed_sync_middleware<E, M>(&self, middleware: M) -> Result<Subscription, EventBusError>
    where
        E: Event,
        M: TypedSyncMiddleware<E>,
    {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }
        let mw = Arc::new(middleware);
        let slot = TypedMiddlewareSlot {
            id: self.next_subscription_id(),
            middleware: TypedMiddlewareEntry::Sync(Arc::new(move |event_name, event| {
                let Some(event) = event.downcast_ref::<E>() else {
                    return crate::middleware::MiddlewareDecision::Reject("event type mismatch".to_string());
                };
                mw.process(event_name, event)
            })),
        };

        let mut registry = self.inner.registry.lock().await;
        registry.add_typed_middleware(TypeId::of::<E>(), std::any::type_name::<E>(), slot.clone());
        self.refresh_snapshot_locked(&registry).await;
        Ok(Subscription::new(slot.id, self.clone()))
    }

    async fn publish_erased(
        &self,
        event_type: TypeId,
        event: Arc<dyn Any + Send + Sync>,
        event_name: &'static str,
        dispatch_ctx: &DispatchContext,
    ) -> Result<(), EventBusError> {
        let snapshot = self.inner.snapshot.load_full();
        let once_removed = dispatch_with_snapshot(&snapshot, event_type, event, event_name, dispatch_ctx).await?;

        if !once_removed.is_empty() {
            let mut registry = self.inner.registry.lock().await;
            for subscription_id in once_removed {
                registry.remove_once(subscription_id);
            }
            self.refresh_snapshot_locked(&registry).await;
        }

        Ok(())
    }

    pub async fn publish<E>(&self, event: E) -> Result<(), EventBusError>
    where
        E: Event + Clone,
    {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }

        let snapshot = self.inner.snapshot.load();
        if !snapshot.by_type.contains_key(&TypeId::of::<E>()) && snapshot.global_middlewares.is_empty() {
            return Ok(());
        }

        let _permit = self.inner.publish_permits.acquire().await.map_err(|_| EventBusError::Stopped)?;
        let dispatch_ctx = self.inner.full_dispatch_context();
        self.publish_erased(TypeId::of::<E>(), Arc::new(event), std::any::type_name::<E>(), &dispatch_ctx)
            .await
    }

    pub fn try_publish<E>(&self, event: E) -> Result<(), EventBusError>
    where
        E: Event + Clone,
    {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }

        let snapshot = self.inner.snapshot.load();
        if !snapshot.by_type.contains_key(&TypeId::of::<E>()) && snapshot.global_middlewares.is_empty() {
            return Ok(());
        }

        let Ok(permit) = Arc::clone(&self.inner.publish_permits).try_acquire_owned() else {
            return Err(EventBusError::ChannelFull);
        };
        let bus = self.clone();
        tokio::spawn(async move {
            let _keep = permit;
            let dispatch_ctx = bus.inner.full_dispatch_context();
            if let Err(err) = bus
                .publish_erased(TypeId::of::<E>(), Arc::new(event), std::any::type_name::<E>(), &dispatch_ctx)
                .await
            {
                error!(error = %err, "event_bus.try_publish.dispatch_failed");
            }
        });
        Ok(())
    }

    pub async fn unsubscribe(&self, subscription_id: SubscriptionId) -> Result<bool, EventBusError> {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }
        let mut registry = self.inner.registry.lock().await;
        let removed = registry.remove_subscription(subscription_id);
        if removed {
            self.refresh_snapshot_locked(&registry).await;
        }
        Ok(removed)
    }

    pub async fn shutdown(&self) -> Result<(), EventBusError> {
        if self
            .inner
            .shutdown_called
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }

        self.inner.publish_permits.close();

        let timed_out = self.inner.tracker.shutdown(self.inner.shutdown_timeout).await;

        let (ack_tx, ack_rx) = oneshot::channel();
        let _ = self.inner.notify_tx.send(ControlNotification::Flush(ack_tx));
        let _ = ack_rx.await;

        let handle = { self.inner.control_handle.lock().expect("control_handle lock poisoned").take() };
        if let Some(handle) = handle {
            handle.abort();
            let _ = handle.await;
        }

        if timed_out { Err(EventBusError::ShutdownTimeout) } else { Ok(()) }
    }

    pub async fn is_healthy(&self) -> bool {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return false;
        }
        let guard = self.inner.control_handle.lock().expect("control_handle lock poisoned");
        match guard.as_ref() {
            Some(handle) => !handle.is_finished(),
            None => false,
        }
    }

    pub async fn stats(&self) -> Result<BusStats, EventBusError> {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }
        let registry = self.inner.registry.lock().await;
        Ok(registry.stats(
            self.inner.tracker.in_flight(),
            self.inner.buffer_size,
            self.inner.shutdown_called.load(Ordering::Acquire),
        ))
    }
}

async fn control_loop(inner: std::sync::Weak<Inner>, mut notify_rx: mpsc::UnboundedReceiver<ControlNotification>) {
    while let Some(notification) = notify_rx.recv().await {
        let Some(inner) = inner.upgrade() else {
            break;
        };
        match notification {
            ControlNotification::Failure(failure) => {
                if let Some(dead_letter) = dead_letter_from_failure(&failure) {
                    let bus = EventBus { inner };
                    let dead_letter_type = std::any::type_name::<DeadLetter>();
                    let dispatch_ctx = bus.inner.sync_only_dispatch_context();
                    let _ = bus
                        .publish_erased(TypeId::of::<DeadLetter>(), Arc::new(dead_letter), dead_letter_type, &dispatch_ctx)
                        .await;
                }
            }
            ControlNotification::Flush(ack) => {
                let _ = ack.send(());
            }
        }
    }
    warn!("event_bus.control_loop.stopped");
}
