mod builder;
mod publish;
mod subscribe;

pub use builder::EventBusBuilder;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use crate::error::EventBusError;
use crate::registry::{AsyncTaskTracker, DispatchContext, MutableRegistry, RegistrySnapshot};
use crate::types::{BusConfig, BusStats, SubscriptionDefaults, SubscriptionId};
use arc_swap::ArcSwap;
use tokio::sync::{Mutex, Notify, Semaphore};

const DISPATCH_CLOSED_BIT: usize = 1usize << (usize::BITS - 1);
const DISPATCH_COUNT_MASK: usize = DISPATCH_CLOSED_BIT - 1;

pub(super) struct Inner {
    pub(super) snapshot: ArcSwap<RegistrySnapshot>,
    pub(super) registry: Mutex<MutableRegistry>,
    pub(super) subscription_defaults: SubscriptionDefaults,
    pub(super) tracker: Arc<AsyncTaskTracker>,
    pub(super) async_limit: Option<Arc<Semaphore>>,
    pub(super) shutdown_timeout: Option<Duration>,
    pub(super) handler_timeout: Option<Duration>,
    pub(super) dispatch_state: AtomicUsize,
    pub(super) dispatches_drained: Notify,
    pub(super) next_subscription_id: AtomicU64,
}

impl Inner {
    pub(super) fn full_dispatch_context<'a>(&'a self, bus: &'a EventBus) -> DispatchContext<'a> {
        DispatchContext {
            bus,
            spawn_async_handlers: true,
            #[cfg(feature = "trace")]
            parent_span: tracing::Span::current(),
        }
    }

    pub(super) fn sync_only_dispatch_context<'a>(&'a self, bus: &'a EventBus) -> DispatchContext<'a> {
        DispatchContext {
            bus,
            spawn_async_handlers: false,
            #[cfg(feature = "trace")]
            parent_span: tracing::Span::current(),
        }
    }

    pub(super) fn is_stopped(&self) -> bool {
        self.dispatch_state.load(Ordering::Acquire) & DISPATCH_CLOSED_BIT != 0
    }

    pub(super) fn dispatches_in_flight(&self) -> usize {
        self.dispatch_state.load(Ordering::Acquire) & DISPATCH_COUNT_MASK
    }

    pub(super) fn close_dispatches(&self) -> bool {
        loop {
            let state = self.dispatch_state.load(Ordering::Acquire);
            if state & DISPATCH_CLOSED_BIT != 0 {
                return false;
            }
            let new_state = state | DISPATCH_CLOSED_BIT;
            if self
                .dispatch_state
                .compare_exchange_weak(state, new_state, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }
}

/// The central in-process event bus.
///
/// `EventBus` is a cheap-to-clone handle backed by a shared `Arc`. All clones
/// refer to the same underlying runtime state and share the same listener
/// registry, middleware pipeline, and configuration.
///
/// Use [`EventBus::builder()`] to construct and configure a bus.
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
    pub(super) inner: Arc<Inner>,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("subscription_defaults", &self.inner.subscription_defaults)
            .field("shutdown_called", &self.inner.is_stopped())
            .finish()
    }
}

impl EventBus {
    /// Return an [`EventBusBuilder`] for constructing a customised bus.
    pub fn builder() -> EventBusBuilder {
        EventBusBuilder::new()
    }

    pub(super) fn from_config(config: BusConfig) -> Self {
        let tracker = Arc::new(AsyncTaskTracker::new(config.shutdown_timeout.is_some()));
        let inner = Arc::new(Inner {
            snapshot: ArcSwap::from_pointee(RegistrySnapshot::default()),
            registry: Mutex::new(MutableRegistry::new()),
            subscription_defaults: config.subscription_defaults,
            tracker,
            async_limit: config.max_concurrent_async.map(|n| Arc::new(Semaphore::new(n))),
            shutdown_timeout: config.shutdown_timeout,
            handler_timeout: config.handler_timeout,
            dispatch_state: AtomicUsize::new(0),
            dispatches_drained: Notify::new(),
            next_subscription_id: AtomicU64::new(1),
        });

        Self { inner }
    }

    pub(super) fn next_subscription_id(&self) -> SubscriptionId {
        let id = self
            .inner
            .next_subscription_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| current.checked_add(1));
        match id {
            Ok(id) => SubscriptionId(id),
            Err(_) => panic!("subscription ID overflow: exceeded u64::MAX subscriptions"),
        }
    }

    pub(super) async fn refresh_snapshot_locked(&self, registry: &MutableRegistry) {
        self.inner.snapshot.store(Arc::new(registry.snapshot()));
    }

    pub(super) fn begin_dispatch(&self) -> Result<DispatchGuard<'_>, EventBusError> {
        loop {
            let state = self.inner.dispatch_state.load(Ordering::Acquire);
            if state & DISPATCH_CLOSED_BIT != 0 {
                return Err(EventBusError::Stopped);
            }
            let in_flight = state & DISPATCH_COUNT_MASK;
            if in_flight == DISPATCH_COUNT_MASK {
                panic!("dispatch counter overflow: exceeded maximum in-flight publishes");
            }
            if self
                .inner
                .dispatch_state
                .compare_exchange_weak(state, state + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(DispatchGuard { inner: self.inner.as_ref() });
            }
        }
    }

    async fn wait_for_dispatches_to_drain(&self, timeout: Option<Duration>) -> bool {
        if self.inner.dispatches_in_flight() == 0 {
            return false;
        }

        let wait = async {
            loop {
                let notified = self.inner.dispatches_drained.notified();
                if self.inner.dispatches_in_flight() == 0 {
                    return;
                }
                notified.await;
            }
        };

        if let Some(timeout) = timeout {
            tokio::time::timeout(timeout, wait).await.is_err()
        } else {
            wait.await;
            false
        }
    }

    fn remaining_timeout(deadline: Option<tokio::time::Instant>) -> Option<Duration> {
        deadline.map(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()))
    }

    pub(crate) fn try_unsubscribe_best_effort(&self, subscription_id: SubscriptionId) -> bool {
        if self.inner.is_stopped() {
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

    /// Explicitly remove a listener or middleware by its [`SubscriptionId`].
    ///
    /// Returns `Ok(true)` if the subscription was found and removed, `Ok(false)`
    /// if it was already absent.
    ///
    /// Prefer using the [`Subscription`](crate::Subscription) handle returned by the `subscribe_*`
    /// and `add_*middleware*` methods — calling
    /// [`Subscription::unsubscribe`](crate::Subscription::unsubscribe) is
    /// equivalent and does not require storing the id separately.
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::Stopped`] if the bus has already been shut down.
    pub async fn unsubscribe(&self, subscription_id: SubscriptionId) -> Result<bool, EventBusError> {
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
        let mut registry = self.inner.registry.lock().await;
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
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
    /// 2. Accepted dispatch tasks finish middleware and spawn any async listener work.
    /// 3. Tracked async listener tasks are awaited.
    /// 4. If a [`shutdown_timeout`](EventBusBuilder::shutdown_timeout) was
    ///    configured and the deadline passes, remaining async tasks are
    ///    aborted and shutdown returns [`EventBusError::ShutdownTimeout`].
    ///
    /// Calling `shutdown` more than once is a no-op (returns `Ok(())`).
    ///
    /// # Errors
    ///
    /// Returns [`EventBusError::ShutdownTimeout`] if the configured
    /// `shutdown_timeout` elapsed before all async tasks completed.
    pub async fn shutdown(&self) -> Result<(), EventBusError> {
        if !self.inner.close_dispatches() {
            return Ok(());
        }

        let deadline = self.inner.shutdown_timeout.map(|timeout| tokio::time::Instant::now() + timeout);

        let mut timed_out = self.wait_for_dispatches_to_drain(Self::remaining_timeout(deadline)).await;
        if timed_out {
            self.inner.tracker.reject_new_tasks();
            timed_out |= self.inner.tracker.shutdown(Some(Duration::ZERO)).await;
        } else {
            timed_out |= self.inner.tracker.shutdown(Self::remaining_timeout(deadline)).await;
        }

        if timed_out { Err(EventBusError::ShutdownTimeout) } else { Ok(()) }
    }

    /// Return `true` if the bus is running.
    ///
    /// Returns `false` after [`shutdown`](Self::shutdown) has been called.
    /// This performs only an atomic load and does not require `.await`.
    pub fn is_healthy(&self) -> bool {
        !self.inner.is_stopped()
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
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
        let registry = self.inner.registry.lock().await;
        if self.inner.is_stopped() {
            return Err(EventBusError::Stopped);
        }
        Ok(registry.stats(self.inner.tracker.in_flight(), self.inner.dispatches_in_flight(), self.inner.is_stopped()))
    }
}

pub(super) struct DispatchGuard<'a> {
    inner: &'a Inner,
}

impl DispatchGuard<'_> {
    fn finish_inner(inner: &Inner) {
        let prev = inner.dispatch_state.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev & DISPATCH_COUNT_MASK > 0, "dispatch counter underflow");
        if (prev & DISPATCH_COUNT_MASK) == 1 {
            inner.dispatches_drained.notify_waiters();
        }
    }
}

impl Drop for DispatchGuard<'_> {
    fn drop(&mut self) {
        Self::finish_inner(self.inner);
    }
}
