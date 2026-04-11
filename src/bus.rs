use std::any::{Any, TypeId};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use tokio::sync::{Mutex, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;
#[cfg(feature = "trace")]
use tracing::{error, trace, warn};

use crate::error::{ConfigError, EventBusError};
use crate::handler::{IntoHandler, RegisteredHandler, SyncEventHandler};
use crate::middleware::{Middleware, SyncMiddleware, TypedMiddleware, TypedSyncMiddleware};
use crate::registry::{
    AsyncTaskTracker, ControlNotification, DispatchContext, ErasedMiddleware, MutableRegistry, RegistrySnapshot, TypeSlot, TypedMiddlewareEntry,
    TypedMiddlewareSlot, dead_letter_from_failure, dispatch_sync_only_with_snapshot, dispatch_with_snapshot,
};
use crate::subscription::Subscription;
use crate::types::{BusConfig, BusStats, DeadLetter, Event, IntoSubscriptionPolicy, SubscriptionId, SubscriptionPolicy, SyncSubscriptionPolicy};

/// Builder for constructing an [`EventBus`] with custom configuration.
///
/// Obtain an instance via [`EventBus::builder()`]. All settings have sensible
/// defaults (see individual methods), so calling [`build`](Self::build) on a
/// freshly created builder is valid and yields a ready-to-use bus.
///
/// # Errors
///
/// [`build`](Self::build) returns [`EventBusError::InvalidConfig`] when:
/// - `buffer_size` was set to `0`.
/// - `max_concurrent_async` was set to `0`.
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

    /// Set the internal channel buffer capacity.
    ///
    /// This controls the maximum number of in-flight publish permits at any
    /// given time. When the buffer is full, [`EventBus::publish`] will wait
    /// until space becomes available, and [`EventBus::try_publish`] will
    /// return [`EventBusError::ChannelFull`] immediately.
    ///
    /// **Default:** `256`.
    ///
    /// # Errors
    ///
    /// [`build`](Self::build) will return an error if `size` is `0`.
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.config.buffer_size = size;
        self
    }

    /// Set a per-invocation timeout for async handler tasks.
    ///
    /// If an async handler does not complete within this duration it is
    /// cancelled and treated as a failure (subject to the listener's
    /// [`SubscriptionPolicy`]). Sync handlers are not affected.
    ///
    /// **Default:** no timeout (handlers may run indefinitely).
    pub fn handler_timeout(mut self, timeout: Duration) -> Self {
        self.config.handler_timeout = Some(timeout);
        self
    }

    /// Set the maximum number of async handler tasks that may run concurrently.
    ///
    /// When this limit is reached, new async dispatches wait until a running
    /// task finishes. Sync handlers are not counted toward this limit.
    ///
    /// **Default:** unlimited (`None`).
    ///
    /// # Errors
    ///
    /// [`build`](Self::build) will return an error if `max` is `0`.
    pub fn max_concurrent_async(mut self, max: usize) -> Self {
        self.config.max_concurrent_async = Some(max);
        self
    }

    /// Set the fallback [`SubscriptionPolicy`] applied to every new subscription
    /// that does not specify its own policy.
    ///
    /// This policy is overridden on a per-subscription basis by
    /// [`EventBus::subscribe_with_policy`] and friends.
    ///
    /// **Default:** [`SubscriptionPolicy::default()`] — priority `0`, no
    /// retries, dead-letter enabled.
    pub fn default_subscription_policy(mut self, policy: SubscriptionPolicy) -> Self {
        self.config.default_subscription_policy = policy;
        self
    }

    #[deprecated(since = "0.3.3", note = "renamed to default_subscription_policy")]
    pub fn default_failure_policy(self, policy: SubscriptionPolicy) -> Self {
        self.default_subscription_policy(policy)
    }

    /// Set a deadline for draining in-flight async tasks during shutdown.
    ///
    /// If tasks do not complete within this duration after [`EventBus::shutdown`]
    /// is called they are aborted and shutdown returns
    /// [`EventBusError::ShutdownTimeout`].
    ///
    /// **Default:** no timeout (shutdown waits indefinitely for tasks to
    /// finish).
    pub fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.config.shutdown_timeout = Some(timeout);
        self
    }

    /// Consume the builder and construct the [`EventBus`].
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::InvalidConfig`] wrapping a [`ConfigError`] if:
    /// - `buffer_size` is `0` ([`ConfigError::ZeroBufferSize`]).
    /// - `max_concurrent_async` is `0` ([`ConfigError::ZeroConcurrency`]).
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
    default_subscription_policy: SubscriptionPolicy,
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
    fn full_dispatch_context(&self) -> DispatchContext<'_> {
        DispatchContext {
            tracker: &self.tracker,
            notify_tx: &self.notify_tx,
            handler_timeout: self.handler_timeout,
            spawn_async_handlers: true,
        }
    }

    fn sync_only_dispatch_context(&self) -> DispatchContext<'_> {
        DispatchContext {
            tracker: &self.tracker,
            notify_tx: &self.notify_tx,
            handler_timeout: self.handler_timeout,
            spawn_async_handlers: false,
        }
    }
}

/// The central in-process event bus.
///
/// `EventBus` is a cheap-to-clone handle backed by a shared `Arc`. All clones
/// refer to the same underlying runtime state and share the same listener
/// registry, middleware pipeline, and configuration.
///
/// Use [`EventBus::builder()`] for full configuration or [`EventBus::new`] for
/// a quick default setup.
///
/// # Thread safety
///
/// `EventBus` is `Clone + Send + Sync` and can be freely shared across threads
/// and tasks.
///
/// # Shutdown
///
/// Call [`shutdown`](Self::shutdown) to gracefully stop the bus. After
/// shutdown, all publish and subscribe operations return
/// [`EventBusError::Stopped`].
#[derive(Clone)]
pub struct EventBus {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("default_subscription_policy", &self.inner.default_subscription_policy)
            .field("shutdown_called", &self.inner.shutdown_called.load(Ordering::Relaxed))
            .finish()
    }
}

impl EventBus {
    /// Create an `EventBus` with the given channel buffer size and default
    /// settings for all other options.
    ///
    /// This is a convenience shorthand for
    /// `EventBus::builder().buffer_size(buffer).build()`.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::InvalidConfig`] if `buffer` is `0`.
    pub fn new(buffer: usize) -> Result<Self, EventBusError> {
        Self::builder().buffer_size(buffer).build()
    }

    /// Return an [`EventBusBuilder`] for constructing a customised bus.
    pub fn builder() -> EventBusBuilder {
        EventBusBuilder::new()
    }

    fn from_config(config: BusConfig) -> Self {
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let tracker = Arc::new(AsyncTaskTracker::new(config.shutdown_timeout.is_some()));
        let inner = Arc::new(Inner {
            snapshot: ArcSwap::from_pointee(RegistrySnapshot::default()),
            registry: Mutex::new(MutableRegistry::new(config.max_concurrent_async)),
            default_subscription_policy: config.default_subscription_policy,
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

    /// Register a handler for event type `E` using the bus's default
    /// subscription policy.
    ///
    /// The dispatch mode (async vs sync) is inferred from the handler type:
    /// - Types implementing [`EventHandler<E>`](crate::EventHandler) are dispatched
    ///   asynchronously — `publish` spawns a task and may return before the handler
    ///   finishes. The event type must be `Clone` because a separate clone is passed
    ///   to each concurrent invocation.
    /// - Types implementing [`SyncEventHandler<E>`](crate::SyncEventHandler) are
    ///   dispatched synchronously — `publish` waits for the handler to return before
    ///   proceeding.
    /// - Plain `async fn(E)` closures/function pointers select async dispatch.
    /// - Plain `fn(&E)` closures/function pointers select sync dispatch.
    ///
    /// Returns a [`Subscription`] handle. The listener remains active until the
    /// subscription is explicitly unsubscribed or the bus is shut down.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn subscribe<E, H, M>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        let registered = handler.into_handler();
        let mut policy = self.inner.default_subscription_policy;
        if registered.is_sync {
            policy.max_retries = 0;
            policy.retry_strategy = None;
        }
        self.subscribe_internal::<E>(registered, policy, false).await
    }

    /// Register a handler for event type `E` with an explicit subscription
    /// policy.
    ///
    /// Behaves the same as [`subscribe`](Self::subscribe) but overrides the
    /// bus-level default [`SubscriptionPolicy`] for this listener.
    ///
    /// The `policy` parameter accepts either a [`SubscriptionPolicy`] (async
    /// handlers only — priority + retry + dead-letter) or a
    /// [`SyncSubscriptionPolicy`] (all handlers — priority + dead-letter only).
    /// Passing a [`SubscriptionPolicy`] for a sync handler is a compile-time
    /// error enforced by [`IntoSubscriptionPolicy`].
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn subscribe_with_policy<E, H, M>(&self, handler: H, policy: impl IntoSubscriptionPolicy<M>) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        let registered = handler.into_handler();
        self.subscribe_internal::<E>(registered, policy.into_subscription_policy(), false).await
    }

    async fn subscribe_internal<E: Event>(
        &self,
        registered: RegisteredHandler,
        subscription_policy: SubscriptionPolicy,
        once: bool,
    ) -> Result<Subscription, EventBusError> {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }

        #[cfg(feature = "trace")]
        trace!("event_bus.subscribe {:?}", &registered.name);
        let id = self.next_subscription_id();
        let listener = (registered.register)(id, subscription_policy, once);

        let mut registry = self.inner.registry.lock().await;
        registry.add_listener(TypeId::of::<E>(), std::any::type_name::<E>(), listener);
        self.refresh_snapshot_locked(&registry).await;

        Ok(Subscription::new(id, registered.name, self.clone()))
    }

    /// Register a sync handler that receives [`DeadLetter`] events.
    ///
    /// Dead-letter listeners must be **synchronous** ([`SyncEventHandler<DeadLetter>`]).
    /// The listener's own `dead_letter` flag is forced to `false` to prevent
    /// infinite recursion (a failure in a dead-letter listener cannot produce
    /// another dead letter).
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn subscribe_dead_letters<H>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        H: SyncEventHandler<DeadLetter>,
    {
        let policy = SyncSubscriptionPolicy::default().with_dead_letter(false);
        self.subscribe_with_policy::<DeadLetter, H, crate::handler::SyncMode>(handler, policy)
            .await
    }

    /// Register a handler that automatically unsubscribes after its first
    /// invocation.
    ///
    /// Uses the bus default subscription policy with `dead_letter` inherited
    /// from that policy. For custom dead-letter behaviour use
    /// [`subscribe_once_with_policy`](Self::subscribe_once_with_policy).
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn subscribe_once<E, H, M>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        let policy = SyncSubscriptionPolicy {
            priority: self.inner.default_subscription_policy.priority,
            dead_letter: self.inner.default_subscription_policy.dead_letter,
        };
        self.subscribe_once_with_policy(handler, policy).await
    }

    /// Register a one-shot handler with an explicit [`SyncSubscriptionPolicy`].
    ///
    /// The handler fires at most once and then unsubscribes itself. Retries
    /// are not supported for one-shot listeners, so only
    /// [`SyncSubscriptionPolicy`] is accepted.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn subscribe_once_with_policy<E, H, M>(
        &self,
        handler: H,
        subscription_policy: SyncSubscriptionPolicy,
    ) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        let registered = handler.into_handler();
        self.subscribe_internal::<E>(registered, subscription_policy.into(), true).await
    }

    /// Add a global async middleware that intercepts **all** event types.
    ///
    /// Middleware runs before any listener receives an event. If the middleware
    /// returns [`MiddlewareDecision::Reject`](crate::MiddlewareDecision::Reject),
    /// the event is dropped and [`EventBusError::MiddlewareRejected`] is returned
    /// to the caller.
    ///
    /// Global middlewares run in registration order. Returns a [`Subscription`]
    /// that can be used to remove the middleware via [`unsubscribe`](Self::unsubscribe).
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
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
        Ok(Subscription::new(id, None, self.clone()))
    }

    /// Add a global **sync** middleware that intercepts all event types.
    ///
    /// Behaves like [`add_middleware`](Self::add_middleware) but the middleware
    /// function is synchronous and runs inline during dispatch.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
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
        Ok(Subscription::new(id, None, self.clone()))
    }

    /// Add an async middleware scoped to a single event type `E`.
    ///
    /// Unlike [`add_middleware`](Self::add_middleware), typed middleware only
    /// intercepts events of type `E`. Multiple typed middlewares for the same
    /// type run in registration order.
    ///
    /// Returns a [`Subscription`] that can be used to remove the middleware.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
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
        Ok(Subscription::new(slot.id, None, self.clone()))
    }

    /// Add a **sync** middleware scoped to a single event type `E`.
    ///
    /// Behaves like [`add_typed_middleware`](Self::add_typed_middleware) but the
    /// middleware function is synchronous and runs inline during dispatch.
    ///
    /// Returns a [`Subscription`] that can be used to remove the middleware.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
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
        Ok(Subscription::new(slot.id, None, self.clone()))
    }

    async fn publish_erased(
        &self,
        snapshot: &Arc<RegistrySnapshot>,
        slot: Option<&Arc<TypeSlot>>,
        event: Arc<dyn Any + Send + Sync>,
        event_name: &'static str,
        dispatch_ctx: &DispatchContext<'_>,
    ) -> Result<(), EventBusError> {
        let once_removed = dispatch_with_snapshot(snapshot, slot, event, event_name, dispatch_ctx).await?;

        if !once_removed.is_empty() {
            let mut registry = self.inner.registry.lock().await;
            for subscription_id in once_removed {
                registry.remove_once(subscription_id);
            }
            self.refresh_snapshot_locked(&registry).await;
        }

        Ok(())
    }

    async fn publish_sync_only<E>(
        &self,
        snapshot: &Arc<RegistrySnapshot>,
        slot: Option<&Arc<TypeSlot>>,
        event: E,
        event_name: &'static str,
    ) -> Result<(), EventBusError>
    where
        E: Event + Clone,
    {
        let once_removed = dispatch_sync_only_with_snapshot(snapshot, slot, &event, event_name, &self.inner.notify_tx).await?;

        if !once_removed.is_empty() {
            let mut registry = self.inner.registry.lock().await;
            for subscription_id in once_removed {
                registry.remove_once(subscription_id);
            }
            self.refresh_snapshot_locked(&registry).await;
        }

        Ok(())
    }

    /// Publish an event to all registered listeners.
    ///
    /// Dispatch behaviour depends on handler type:
    /// - **Sync handlers** run inline; `publish` waits for each one to return.
    /// - **Async handlers** are spawned as separate tasks; `publish` returns
    ///   once all tasks have been *spawned*, not necessarily *completed*.
    ///
    /// If the internal channel buffer is full this method waits asynchronously
    /// until capacity is available. Use [`try_publish`](Self::try_publish) for
    /// a non-blocking alternative.
    ///
    /// Events with no registered listeners (and no global middleware) are
    /// silently dropped without allocating.
    ///
    /// # Errors
    ///
    /// - [`EventBusError::Stopped`] — the bus has been shut down.
    /// - [`EventBusError::MiddlewareRejected`] — a middleware rejected the event.
    pub async fn publish<E>(&self, event: E) -> Result<(), EventBusError>
    where
        E: Event + Clone,
    {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }

        let snapshot = self.inner.snapshot.load_full();
        let event_type = TypeId::of::<E>();
        let slot = snapshot.by_type.get(&event_type);
        if slot.is_none() && snapshot.global_middlewares.is_empty() {
            return Ok(());
        }

        let _permit = self.inner.publish_permits.acquire().await.map_err(|_| EventBusError::Stopped)?;
        let sync_only =
            !snapshot.global_has_async_middleware && slot.is_none_or(|slot| slot.async_listeners.is_empty() && !slot.has_async_middleware);

        if sync_only {
            self.publish_sync_only(&snapshot, slot, event, std::any::type_name::<E>()).await
        } else {
            let dispatch_ctx = self.inner.full_dispatch_context();
            self.publish_erased(&snapshot, slot, Arc::new(event), std::any::type_name::<E>(), &dispatch_ctx)
                .await
        }
    }

    /// Attempt to publish an event without waiting for buffer capacity.
    ///
    /// If there is room in the internal channel the event is enqueued and
    /// dispatched in a background task; otherwise
    /// [`EventBusError::ChannelFull`] is returned immediately.
    ///
    /// Because dispatch happens in a background task, errors from individual
    /// listeners are logged via `tracing` but are **not** propagated to the
    /// caller. Use [`publish`](Self::publish) if you need to observe per-listener
    /// errors or middleware rejections synchronously.
    ///
    /// # Errors
    ///
    /// - [`EventBusError::Stopped`] — the bus has been shut down.
    /// - [`EventBusError::ChannelFull`] — no buffer space is available.
    pub fn try_publish<E>(&self, event: E) -> Result<(), EventBusError>
    where
        E: Event + Clone,
    {
        if self.inner.shutdown_called.load(Ordering::Acquire) {
            return Err(EventBusError::Stopped);
        }

        let snapshot = self.inner.snapshot.load_full();
        let event_type = TypeId::of::<E>();
        let slot = snapshot.by_type.get(&event_type);
        if slot.is_none() && snapshot.global_middlewares.is_empty() {
            return Ok(());
        }
        let sync_only =
            !snapshot.global_has_async_middleware && slot.is_none_or(|slot| slot.async_listeners.is_empty() && !slot.has_async_middleware);

        let Ok(permit) = Arc::clone(&self.inner.publish_permits).try_acquire_owned() else {
            return Err(EventBusError::ChannelFull);
        };
        let slot = slot.cloned();

        let bus = self.clone();
        tokio::spawn(async move {
            let _keep = permit;
            let slot = slot.as_ref();
            if sync_only {
                if let Err(_err) = bus.publish_sync_only(&snapshot, slot, event, std::any::type_name::<E>()).await {
                    #[cfg(feature = "trace")]
                    error!(error = %_err, "event_bus.try_publish.dispatch_failed");
                }
            } else {
                let dispatch_ctx = bus.inner.full_dispatch_context();
                if let Err(_err) = bus
                    .publish_erased(&snapshot, slot, Arc::new(event), std::any::type_name::<E>(), &dispatch_ctx)
                    .await
                {
                    #[cfg(feature = "trace")]
                    error!(error = %_err, "event_bus.try_publish.dispatch_failed");
                }
            }
        });
        Ok(())
    }

    /// Explicitly remove a listener or middleware by its [`SubscriptionId`].
    ///
    /// Returns `Ok(true)` if the subscription was found and removed, `Ok(false)`
    /// if it was already absent.
    ///
    /// Prefer using the [`Subscription`] handle returned by the `subscribe_*`
    /// and `add_*middleware*` methods — calling
    /// [`Subscription::unsubscribe`](crate::Subscription::unsubscribe) is
    /// equivalent and does not require storing the id separately.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
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

    /// Gracefully shut down the bus.
    ///
    /// Shutdown proceeds in the following order:
    /// 1. The bus is marked as stopped; subsequent publish/subscribe calls
    ///    return [`EventBusError::Stopped`].
    /// 2. The internal publish-permit semaphore is closed so no new dispatches
    ///    can begin.
    /// 3. In-flight async handler tasks are awaited. If a
    ///    [`shutdown_timeout`](EventBusBuilder::shutdown_timeout) was configured
    ///    and the deadline passes, remaining tasks are aborted.
    /// 4. Any pending failure/dead-letter notifications are flushed before the
    ///    internal control loop terminates.
    ///
    /// Calling `shutdown` more than once is a no-op (returns `Ok(())`).
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::ShutdownTimeout`] if the configured
    /// `shutdown_timeout` elapsed before all async tasks completed.
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

    /// Return `true` if the bus is running and its internal control loop is
    /// alive.
    ///
    /// Returns `false` after [`shutdown`](Self::shutdown) has been called, or
    /// if the control loop has unexpectedly stopped.
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

    /// Return a point-in-time snapshot of the bus internal state as
    /// [`BusStats`].
    ///
    /// Useful for observability, health checks, and debugging. The snapshot is
    /// computed under a brief registry lock and reflects the state at the
    /// moment of the call.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
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
                    let snapshot = bus.inner.snapshot.load_full();
                    let slot = snapshot.by_type.get(&TypeId::of::<DeadLetter>());
                    let _ = bus
                        .publish_erased(&snapshot, slot, Arc::new(dead_letter), dead_letter_type, &dispatch_ctx)
                        .await;
                }
            }
            ControlNotification::Flush(ack) => {
                let _ = ack.send(());
            }
        }
    }
    #[cfg(feature = "trace")]
    warn!("event_bus.control_loop.stopped");
}
