// SPDX-License-Identifier: MIT
use std::any::TypeId;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tracing::{error, trace};

use crate::actor::{BusMessage, EventBusActor};
use crate::error::EventBusError;
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
            return Err(EventBusError::InvalidConfig("buffer_size must be greater than zero".into()));
        }
        if self.config.max_concurrent_async == Some(0) {
            return Err(EventBusError::InvalidConfig("max_concurrent_async must be greater than zero".into()));
        }
        Ok(EventBus::from_config(self.config))
    }
}

#[derive(Clone)]
pub struct EventBus {
    tx: mpsc::Sender<BusMessage>,
    default_failure_policy: FailurePolicy,
}

impl EventBus {
    /// Create an event bus with the given channel buffer size and default
    /// settings.
    ///
    /// # Panics
    ///
    /// Panics if `buffer` is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use jaeb::EventBus;
    ///
    /// # #[tokio::main] async fn main() {
    /// let bus = EventBus::new(256);
    /// bus.shutdown().await.unwrap();
    /// # }
    /// ```
    pub fn new(buffer: usize) -> Self {
        assert!(buffer > 0, "buffer size must be greater than zero");
        Self::from_config(BusConfig {
            buffer_size: buffer,
            ..BusConfig::default()
        })
    }

    /// Return a builder for fine-grained configuration.
    pub fn builder() -> EventBusBuilder {
        EventBusBuilder::new()
    }

    fn from_config(config: BusConfig) -> Self {
        let (tx, rx) = mpsc::channel(config.buffer_size);
        let default_failure_policy = config.default_failure_policy;
        let actor = EventBusActor::new(tx.clone(), rx, &config);
        tokio::spawn(actor.run());
        Self { tx, default_failure_policy }
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
    /// let bus = EventBus::new(64);
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
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use jaeb::{EventBus, FailurePolicy, SyncEventHandler, HandlerResult};
    ///
    /// #[derive(Clone)]
    /// struct Job(u32);
    ///
    /// struct Worker;
    /// impl SyncEventHandler<Job> for Worker {
    ///     fn handle(&self, _event: &Job) -> HandlerResult { Ok(()) }
    /// }
    ///
    /// # #[tokio::main] async fn main() {
    /// let bus = EventBus::new(64);
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

        let subscription_id = self
            .send_and_ack(|ack| BusMessage::Subscribe {
                event_type: TypeId::of::<E>(),
                handler: registered.erased,
                mode: registered.mode,
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
    /// let bus = EventBus::new(64);
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
            event: Box::new(event),
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
    /// let bus = EventBus::new(1);
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
            event: Box::new(event),
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
    /// Calling `shutdown` a second time will return
    /// [`EventBusError::ActorStopped`] because the actor is already gone.
    /// This makes shutdown effectively idempotent — the first call performs
    /// the work, and subsequent calls are safe no-ops that return an error.
    ///
    /// # Examples
    ///
    /// ```
    /// use jaeb::EventBus;
    ///
    /// # #[tokio::main] async fn main() {
    /// let bus = EventBus::new(64);
    /// bus.shutdown().await.unwrap();
    ///
    /// // Further operations fail:
    /// assert!(bus.publish("late").await.is_err());
    /// # }
    /// ```
    pub async fn shutdown(&self) -> Result<(), EventBusError> {
        trace!("event_bus.shutdown");

        self.send_and_ack(|ack| BusMessage::Shutdown { ack }).await?
    }
}
