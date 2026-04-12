mod builder;
mod publish;
mod subscribe;

pub use builder::EventBusBuilder;

use std::any::TypeId;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use tokio::sync::{Mutex, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;
#[cfg(feature = "trace")]
use tracing::warn;

use crate::error::EventBusError;
use crate::registry::{AsyncTaskTracker, ControlNotification, DispatchContext, MutableRegistry, RegistrySnapshot, dead_letter_from_failure};
use crate::types::{BusConfig, BusStats, DeadLetter, SubscriptionId, SubscriptionPolicy};

pub(super) struct Inner {
    pub(super) snapshot: ArcSwap<RegistrySnapshot>,
    pub(super) registry: Mutex<MutableRegistry>,
    pub(super) default_subscription_policy: SubscriptionPolicy,
    pub(super) tracker: Arc<AsyncTaskTracker>,
    pub(super) publish_permits: Arc<Semaphore>,
    pub(super) shutdown_timeout: Option<Duration>,
    pub(super) handler_timeout: Option<Duration>,
    pub(super) buffer_size: usize,
    pub(super) shutdown_called: AtomicBool,
    pub(super) next_subscription_id: AtomicU64,
    pub(super) notify_tx: mpsc::UnboundedSender<ControlNotification>,
    pub(super) control_handle: StdMutex<Option<JoinHandle<()>>>,
}

impl Inner {
    pub(super) fn full_dispatch_context(&self) -> DispatchContext<'_> {
        DispatchContext {
            tracker: &self.tracker,
            notify_tx: &self.notify_tx,
            handler_timeout: self.handler_timeout,
            spawn_async_handlers: true,
            #[cfg(feature = "trace")]
            parent_span: tracing::Span::current(),
        }
    }

    pub(super) fn sync_only_dispatch_context(&self) -> DispatchContext<'_> {
        DispatchContext {
            tracker: &self.tracker,
            notify_tx: &self.notify_tx,
            handler_timeout: self.handler_timeout,
            spawn_async_handlers: false,
            #[cfg(feature = "trace")]
            parent_span: tracing::Span::current(),
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
            .field("default_subscription_policy", &self.inner.default_subscription_policy)
            .field("shutdown_called", &self.inner.shutdown_called.load(Ordering::Relaxed))
            .finish()
    }
}

impl EventBus {
    /// Return an [`EventBusBuilder`] for constructing a customised bus.
    pub fn builder() -> EventBusBuilder {
        EventBusBuilder::new()
    }

    pub(super) fn from_config(config: BusConfig) -> Self {
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let tracker = Arc::new(AsyncTaskTracker::new(config.shutdown_timeout.is_some()));
        let inner = Arc::new(Inner {
            snapshot: ArcSwap::from_pointee(RegistrySnapshot::default()),
            registry: Mutex::new(MutableRegistry::new(config.max_concurrent_async, Arc::clone(&tracker))),
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
            self.inner.publish_permits.available_permits(),
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
                    // Note: the `parent_span` captured by `sync_only_dispatch_context()` is the
                    // control loop's ambient span, not the original publisher's span. This is
                    // acceptable because dead-letter dispatch only invokes sync handlers (which
                    // run inline and do not use the propagated span). If async dead-letter
                    // handlers are ever supported, the original publisher's span should be
                    // threaded through `ListenerFailure` for full trace lineage.
                    let dispatch_ctx = bus.inner.sync_only_dispatch_context();
                    let snapshot = bus.inner.snapshot.load_full();
                    let slot = snapshot.by_type.get(&TypeId::of::<DeadLetter>());
                    let _ = bus
                        .publish_erased(snapshot.as_ref(), slot, Arc::new(dead_letter), dead_letter_type, &dispatch_ctx)
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
