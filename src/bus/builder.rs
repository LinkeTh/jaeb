use std::time::Duration;

use crate::deps::Deps;
use crate::error::{ConfigError, EventBusError};
use crate::handler::{DeadLetterDescriptor, HandlerDescriptor};
use crate::types::{BusConfig, SubscriptionPolicy};

use super::EventBus;

/// Builder for constructing an [`EventBus`] with custom configuration and
/// pre-registered handlers.
///
/// Obtain an instance via [`EventBus::builder()`]. All settings have sensible
/// defaults (see individual methods), so calling [`build`](Self::build) on a
/// freshly created builder is valid and yields a ready-to-use bus.
///
/// # Handler registration
///
/// Use [`handler`](Self::handler) and [`dead_letter`](Self::dead_letter) to
/// register handlers that will be subscribed automatically during
/// [`build`](Self::build). Dependencies required by those handlers are supplied
/// via [`deps`](Self::deps).
///
/// ```rust,ignore
/// let bus = EventBus::builder()
///     .buffer_size(256)
///     .handler(ProcessPaymentHandler)
///     .handler(AuditLogHandler)
///     .dead_letter(LogDeadLetterHandler)
///     .deps(Deps::new().insert(db).insert(mailer))
///     .build()
///     .await?;
/// ```
///
/// # Errors
///
/// [`build`](Self::build) returns [`EventBusError::InvalidConfig`] when:
/// - `buffer_size` was set to `0`.
/// - `max_concurrent_async` was set to `0`.
///
/// [`build`](Self::build) returns [`EventBusError::MissingDependency`] when a
/// registered handler requires a dependency that was not supplied via
/// [`deps`](Self::deps).
pub struct EventBusBuilder {
    config: BusConfig,
    handlers: Vec<Box<dyn HandlerDescriptor>>,
    dead_letter_handlers: Vec<Box<dyn DeadLetterDescriptor>>,
    deps: Deps,
}

impl std::fmt::Debug for EventBusBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBusBuilder")
            .field("config", &self.config)
            .field("handlers", &self.handlers.len())
            .field("dead_letter_handlers", &self.dead_letter_handlers.len())
            .field("deps", &self.deps)
            .finish()
    }
}

impl EventBusBuilder {
    pub(super) fn new() -> Self {
        Self {
            config: BusConfig::default(),
            handlers: Vec::new(),
            dead_letter_handlers: Vec::new(),
            deps: Deps::new(),
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

    /// Register a handler to be subscribed during [`build`](Self::build).
    ///
    /// The handler's [`HandlerDescriptor::register`] method is called once with
    /// the newly-created bus and the [`Deps`] container provided via
    /// [`deps`](Self::deps). Any required dependencies must be present in `Deps`
    /// by the time `build` is called or it will return
    /// [`EventBusError::MissingDependency`].
    ///
    /// **Lifetime:** The handler's subscription is held by the bus for its
    /// entire lifetime. Builder-registered handlers cannot be individually
    /// unsubscribed after [`build`](Self::build) returns.
    ///
    /// **Note:** This method accepts `impl HandlerDescriptor`. Passing a
    /// dead-letter handler (which only implements [`DeadLetterDescriptor`]) is a
    /// compile-time error — use [`dead_letter`](Self::dead_letter) instead.
    pub fn handler(mut self, handler: impl HandlerDescriptor) -> Self {
        self.handlers.push(Box::new(handler));
        self
    }

    /// Register a dead-letter handler to be subscribed during [`build`](Self::build).
    ///
    /// The handler's [`DeadLetterDescriptor::register_dead_letter`] method is
    /// called once. Dead-letter handlers must implement
    /// [`SyncEventHandler<DeadLetter>`](crate::SyncEventHandler) — passing an
    /// async handler is a compile-time error.
    ///
    /// **Lifetime:** The handler's subscription is held by the bus for its
    /// entire lifetime. Builder-registered dead-letter handlers cannot be
    /// individually unsubscribed after [`build`](Self::build) returns.
    ///
    /// **Note:** Passing a regular handler that only implements
    /// [`HandlerDescriptor`] (not [`DeadLetterDescriptor`]) is a compile-time
    /// error — use [`handler`](Self::handler) instead.
    pub fn dead_letter(mut self, handler: impl DeadLetterDescriptor) -> Self {
        self.dead_letter_handlers.push(Box::new(handler));
        self
    }

    /// Supply the dependency container used to inject dependencies into
    /// registered handlers.
    ///
    /// Call [`Deps::new()`] and chain [`insert`](Deps::insert) calls to build
    /// the container:
    ///
    /// ```rust,ignore
    /// .deps(Deps::new().insert(db_pool).insert(mailer))
    /// ```
    ///
    /// **Default:** an empty [`Deps`] container (suitable when no handlers
    /// require dependencies).
    pub fn deps(mut self, deps: Deps) -> Self {
        self.deps = deps;
        self
    }

    /// Consume the builder, construct the [`EventBus`], and register all
    /// handlers.
    ///
    /// This method:
    /// 1. Validates configuration (returns [`EventBusError::InvalidConfig`] on
    ///    invalid settings).
    /// 2. Creates the [`EventBus`] runtime.
    /// 3. Calls [`HandlerDescriptor::register`] for each handler added via
    ///    [`handler`](Self::handler), in registration order.
    /// 4. Calls [`DeadLetterDescriptor::register_dead_letter`] for each handler
    ///    added via [`dead_letter`](Self::dead_letter), in registration order.
    ///
    /// # Errors
    ///
    /// - [`EventBusError::InvalidConfig`] — `buffer_size` or
    ///   `max_concurrent_async` is `0`.
    /// - [`EventBusError::MissingDependency`] — a registered handler requires a
    ///   dependency that was not supplied via [`deps`](Self::deps).
    /// - [`EventBusError::Stopped`] — should not occur during build, but
    ///   propagated from subscription calls for completeness.
    pub async fn build(self) -> Result<EventBus, EventBusError> {
        if self.config.buffer_size == 0 {
            return Err(ConfigError::ZeroBufferSize.into());
        }
        if self.config.max_concurrent_async == Some(0) {
            return Err(ConfigError::ZeroConcurrency.into());
        }

        let bus = EventBus::from_config(self.config);

        for descriptor in &self.handlers {
            let _ = descriptor.register(&bus, &self.deps).await?;
        }
        for descriptor in &self.dead_letter_handlers {
            let _ = descriptor.register_dead_letter(&bus, &self.deps).await?;
        }

        Ok(bus)
    }
}
