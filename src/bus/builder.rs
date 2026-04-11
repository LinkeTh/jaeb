use std::time::Duration;

use crate::error::{ConfigError, EventBusError};
use crate::types::{BusConfig, SubscriptionPolicy};

use super::EventBus;

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
    pub(super) fn new() -> Self {
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
