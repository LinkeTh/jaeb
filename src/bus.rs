// SPDX-License-Identifier: MIT
use std::any::TypeId;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{error, trace, warn};

use crate::actor::{BusMessage, EventBusActor};
use crate::error::{ConfigError, EventBusError};
use crate::handler::{IntoHandler, SyncEventHandler};
use crate::subscription::Subscription;
use crate::types::{BusConfig, DeadLetter, Event, FailurePolicy};

/// Builder for configuring and constructing an [`EventBus`].
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use jaeb::{EventBus, FailurePolicy};
///
/// # #[tokio::main] async fn main() {
/// let bus = EventBus::builder()
///     .buffer_size(512)
///     .handler_timeout(Duration::from_secs(5))
///     .max_concurrent_async(100)
///     .shutdown_timeout(Duration::from_secs(10))
///     .default_failure_policy(
///         FailurePolicy::default().with_max_retries(2),
///     )
///     .build()
///     .expect("valid config");
///
/// bus.shutdown().await.unwrap();
/// # }
/// ```
pub struct EventBusBuilder {
    config: BusConfig,
}

impl EventBusBuilder {
    fn new() -> Self {
        Self {
            config: BusConfig::default(),
        }
    }

    /// Set the internal channel buffer size.
    ///
    /// Must be greater than zero.
    ///
    /// Defaults to `256`.
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.config.buffer_size = size;
        self
    }

    /// Set the maximum time a single handler invocation may run before being
    /// treated as a failure.
    ///
    /// When a handler exceeds this timeout, the attempt is treated as an error
    /// and is eligible for retry or dead-lettering according to the listener's
    /// [`FailurePolicy`].
    ///
    /// **Note:** This timeout only applies to **async** handlers. Sync handlers
    /// execute inline on the actor task and complete immediately from Tokio's
    /// perspective, so the timeout has no practical effect on them.
    ///
    /// By default there is no timeout.
    pub fn handler_timeout(mut self, timeout: Duration) -> Self {
        self.config.handler_timeout = Some(timeout);
        self
    }

    /// Set the maximum number of async handler tasks that may execute
    /// concurrently.
    ///
    /// Must be greater than zero. A value of zero would permanently block all
    /// async handlers.
    ///
    /// When the limit is reached, new async tasks will wait for a permit
    /// before starting execution. Sync handlers are not affected.
    ///
    /// By default there is no limit.
    pub fn max_concurrent_async(mut self, max: usize) -> Self {
        self.config.max_concurrent_async = Some(max);
        self
    }

    /// Set the default [`FailurePolicy`] applied to subscriptions that do not
    /// specify one explicitly.
    pub fn default_failure_policy(mut self, policy: FailurePolicy) -> Self {
        self.config.default_failure_policy = policy;
        self
    }

    /// Set the maximum time [`EventBus::shutdown`] will wait for in-flight
    /// async tasks to complete.
    ///
    /// After this timeout, remaining tasks are aborted. By default, shutdown
    /// waits indefinitely.
    pub fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.config.shutdown_timeout = Some(timeout);
        self
    }

    /// Build and start the [`EventBus`].
    ///
    /// Returns [`EventBusError::InvalidConfig`] if the configuration is
    /// invalid (e.g., zero `buffer_size` or zero `max_concurrent_async`).
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

#[derive(Clone)]
pub struct EventBus {
    tx: mpsc::Sender<BusMessage>,
    default_failure_policy: FailurePolicy,
    /// Handle to the internal actor task.
    ///
    /// Wrapped in `Arc<Mutex<Option<…>>>` so that:
    /// - `EventBus` can remain `Clone` (the handle is shared).
    /// - `shutdown` can `take()` the handle to join the actor exactly once.
    /// - `is_healthy` can peek without consuming the handle.
    actor_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// Tracks whether `shutdown` has been called at least once.
    shutdown_called: Arc<AtomicBool>,
}

impl EventBus {
    /// Create an event bus with the given channel buffer size and default
    /// settings.
    ///
    /// Returns [`EventBusError::InvalidConfig`] if `buffer` is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use jaeb::EventBus;
    ///
    /// # #[tokio::main] async fn main() {
    /// let bus = EventBus::new(256).expect("valid config");
    /// bus.shutdown().await.unwrap();
    /// # }
    /// ```
    pub fn new(buffer: usize) -> Result<Self, EventBusError> {
        Self::builder().buffer_size(buffer).build()
    }

    /// Return a builder for fine-grained configuration.
    pub fn builder() -> EventBusBuilder {
        EventBusBuilder::new()
    }

    fn from_config(config: BusConfig) -> Self {
        let (tx, rx) = mpsc::channel(config.buffer_size);
        let default_failure_policy = config.default_failure_policy;
        let actor = EventBusActor::new(tx.clone(), rx, &config);
        let actor_handle = tokio::spawn(actor.run());
        Self {
            tx,
            default_failure_policy,
            actor_handle: Arc::new(Mutex::new(Some(actor_handle))),
            shutdown_called: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Return a clone of the internal channel sender.
    ///
    /// Used by [`SubscriptionGuard`](crate::subscription::SubscriptionGuard)
    /// to send fire-and-forget unsubscribe messages in its `Drop` impl.
    pub(crate) fn sender(&self) -> mpsc::Sender<BusMessage> {
        self.tx.clone()
    }

    /// Send a message to the actor and wait for the acknowledgement.
    ///
    /// The `make_msg` closure receives a oneshot sender that the actor will
    /// use to reply. Both the send and the ack-wait map failures to
    /// [`EventBusError::ActorStopped`].
    async fn send_and_ack<T>(&self, make_msg: impl FnOnce(oneshot::Sender<T>) -> BusMessage) -> Result<T, EventBusError> {
        let (ack_tx, ack_rx) = oneshot::channel();
        let msg = make_msg(ack_tx);
        let operation = msg.operation_name();
        self.tx.send(msg).await.map_err(|e| {
            error!(operation, error = %e, "event_bus.send_failed");
            EventBusError::ActorStopped
        })?;
        ack_rx.await.map_err(|_| {
            error!(operation, "event_bus.ack_wait_failed");
            EventBusError::ActorStopped
        })
    }

    /// Subscribe a handler for events of type `E`.
    ///
    /// The dispatch mode (async vs sync) is determined automatically by the
    /// handler trait implementation:
    /// - [`EventHandler<E>`](crate::handler::EventHandler) -> async dispatch
    /// - [`SyncEventHandler<E>`](crate::handler::SyncEventHandler) -> sync dispatch
    ///
    /// # Examples
    ///
    /// ```
    /// use jaeb::{EventBus, SyncEventHandler, HandlerResult};
    ///
    /// #[derive(Clone)]
    /// struct MyEvent(String);
    ///
    /// struct Logger;
    /// impl SyncEventHandler<MyEvent> for Logger {
    ///     fn handle(&self, event: &MyEvent) -> HandlerResult {
    ///         println!("got: {}", event.0);
    ///         Ok(())
    ///     }
    /// }
    ///
    /// # #[tokio::main] async fn main() {
    /// let bus = EventBus::new(64).expect("valid config");
    /// let _sub = bus.subscribe::<MyEvent, _, _>(Logger).await.unwrap();
    /// bus.publish(MyEvent("hello".into())).await.unwrap();
    /// bus.shutdown().await.unwrap();
    /// # }
    /// ```
    pub async fn subscribe<E, H, M>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        self.subscribe_with_policy(handler, self.default_failure_policy).await
    }

    /// Subscribe a handler with a custom [`FailurePolicy`].
    ///
    /// See [`subscribe`](Self::subscribe) for dispatch-mode details.
    ///
    /// **Note:** Sync handlers do not support retries. Subscribing a sync
    /// handler with `max_retries > 0` returns
    /// [`EventBusError::SyncRetryNotSupported`]. The `dead_letter` field is
    /// still respected for sync handlers.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use jaeb::{EventBus, EventHandler, FailurePolicy, HandlerResult};
    ///
    /// #[derive(Clone)]
    /// struct Job(u32);
    ///
    /// struct Worker;
    /// impl EventHandler<Job> for Worker {
    ///     async fn handle(&self, _event: &Job) -> HandlerResult { Ok(()) }
    /// }
    ///
    /// # #[tokio::main] async fn main() {
    /// let bus = EventBus::new(64).expect("valid config");
    /// let policy = FailurePolicy::default()
    ///     .with_max_retries(3)
    ///     .with_retry_delay(Duration::from_millis(100));
    /// let _sub = bus
    ///     .subscribe_with_policy::<Job, _, _>(Worker, policy)
    ///     .await
    ///     .unwrap();
    /// bus.shutdown().await.unwrap();
    /// # }
    /// ```
    pub async fn subscribe_with_policy<E, H, M>(&self, handler: H, failure_policy: FailurePolicy) -> Result<Subscription, EventBusError>
    where
        E: Event,
        H: IntoHandler<E, M>,
    {
        trace!("event_bus.subscribe");
        let registered = handler.into_handler();

        // Sync handlers execute exactly once — retries are only supported for
        // async handlers.
        if registered.erased.is_sync() && failure_policy.max_retries > 0 {
            return Err(EventBusError::SyncRetryNotSupported);
        }

        let subscription_id = self
            .send_and_ack(|ack| BusMessage::Subscribe {
                event_type: TypeId::of::<E>(),
                handler: registered.erased,
                failure_policy,
                ack,
            })
            .await?;

        Ok(Subscription::new(subscription_id, self.clone()))
    }

    /// Convenience: subscribe a sync dead-letter handler with `dead_letter: false`
    /// to prevent infinite recursion.
    pub async fn subscribe_dead_letters<H>(&self, handler: H) -> Result<Subscription, EventBusError>
    where
        H: SyncEventHandler<DeadLetter>,
    {
        let policy = FailurePolicy::default().with_dead_letter(false);
        self.subscribe_with_policy::<DeadLetter, H, crate::handler::SyncMode>(handler, policy)
            .await
    }

    /// Publish an event and wait until the actor has dispatched it.
    ///
    /// This waits for synchronous listeners to complete, but may return before
    /// asynchronous listeners finish.
    ///
    /// # Examples
    ///
    /// ```
    /// use jaeb::EventBus;
    ///
    /// #[derive(Clone)]
    /// struct Ping;
    ///
    /// # #[tokio::main] async fn main() {
    /// let bus = EventBus::new(64).expect("valid config");
    /// // Publishing with no listeners is a no-op.
    /// bus.publish(Ping).await.unwrap();
    /// bus.shutdown().await.unwrap();
    /// # }
    /// ```
    pub async fn publish<E>(&self, event: E) -> Result<(), EventBusError>
    where
        E: Event + Clone,
    {
        trace!("event_bus.publish");

        self.send_and_ack(|ack| BusMessage::Publish {
            event_type: TypeId::of::<E>(),
            event: Arc::new(event),
            event_name: std::any::type_name::<E>(),
            ack: Some(ack),
        })
        .await
    }

    /// Try to publish without waiting or blocking.
    ///
    /// Returns [`EventBusError::ChannelFull`] when the internal queue is full.
    ///
    /// # Examples
    ///
    /// ```
    /// use jaeb::{EventBus, EventBusError};
    ///
    /// #[derive(Clone)]
    /// struct Tick;
    ///
    /// # #[tokio::main] async fn main() {
    /// let bus = EventBus::new(1).expect("valid config");
    /// assert!(bus.try_publish(Tick).is_ok());
    /// bus.shutdown().await.unwrap();
    /// # }
    /// ```
    pub fn try_publish<E>(&self, event: E) -> Result<(), EventBusError>
    where
        E: Event + Clone,
    {
        trace!("event_bus.try_publish");

        match self.tx.try_send(BusMessage::Publish {
            event_type: TypeId::of::<E>(),
            event: Arc::new(event),
            event_name: std::any::type_name::<E>(),
            ack: None,
        }) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(EventBusError::ChannelFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(EventBusError::ActorStopped),
        }
    }

    /// Remove a previously registered listener by its [`SubscriptionId`](crate::types::SubscriptionId).
    ///
    /// Returns `Ok(true)` if the listener was found and removed, `Ok(false)` if
    /// no listener with that ID exists (e.g. already unsubscribed), or
    /// `Err(EventBusError::ActorStopped)` if the bus has shut down.
    ///
    /// Prefer using [`Subscription::unsubscribe`](crate::subscription::Subscription::unsubscribe)
    /// when you have the subscription handle.
    pub async fn unsubscribe(&self, subscription_id: crate::types::SubscriptionId) -> Result<bool, EventBusError> {
        trace!(subscription_id = subscription_id.as_u64(), "event_bus.unsubscribe");

        self.send_and_ack(|ack| BusMessage::Unsubscribe { subscription_id, ack }).await
    }

    /// Gracefully stop the actor and wait for queued publish messages plus
    /// in-flight async handlers.
    ///
    /// If a `shutdown_timeout` was configured via the builder, remaining tasks
    /// are aborted after the deadline and [`EventBusError::ShutdownTimeout`] is
    /// returned.
    ///
    /// After shutdown, all publish/subscribe operations return
    /// [`EventBusError::ActorStopped`].
    ///
    /// Shutdown is **idempotent**: the first call performs the work and
    /// subsequent calls return `Ok(())` immediately.
    ///
    /// # Examples
    ///
    /// ```
    /// use jaeb::EventBus;
    ///
    /// # #[tokio::main] async fn main() {
    /// let bus = EventBus::new(64).expect("valid config");
    /// bus.shutdown().await.unwrap();
    ///
    /// // Second shutdown is a no-op:
    /// bus.shutdown().await.unwrap();
    ///
    /// // Further operations fail:
    /// assert!(bus.publish("late").await.is_err());
    /// # }
    /// ```
    pub async fn shutdown(&self) -> Result<(), EventBusError> {
        trace!("event_bus.shutdown");

        // Atomically claim the shutdown responsibility. Only the winner
        // proceeds; all other callers return Ok(()) immediately. This
        // eliminates the TOCTOU race that existed with a separate
        // load-then-store pattern.
        if self
            .shutdown_called
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }

        let result = self.send_and_ack(|ack| BusMessage::Shutdown { ack }).await?;

        // Join the actor task to ensure it has fully exited and to observe
        // any panic that may have occurred after the ack was sent.
        if let Some(handle) = self.actor_handle.lock().await.take()
            && let Err(join_error) = handle.await
        {
            // The actor panicked after sending its ack.  Log the panic but
            // still return the result the actor already committed to — the
            // shutdown itself completed.
            warn!(error = %join_error, "event_bus.actor_panic_during_shutdown");
        }

        result
    }

    /// Returns `true` if the internal actor task is still running.
    ///
    /// This is a best-effort check: it returns `false` if the actor has
    /// exited (either via [`shutdown`](Self::shutdown) or an unexpected panic),
    /// `true` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// use jaeb::EventBus;
    ///
    /// # #[tokio::main] async fn main() {
    /// let bus = EventBus::new(64).expect("valid config");
    /// assert!(bus.is_healthy().await);
    ///
    /// bus.shutdown().await.unwrap();
    /// assert!(!bus.is_healthy().await);
    /// # }
    /// ```
    pub async fn is_healthy(&self) -> bool {
        let guard = self.actor_handle.lock().await;
        match guard.as_ref() {
            Some(handle) => !handle.is_finished(),
            None => false, // handle was already taken by shutdown
        }
    }
}
