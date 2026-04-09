// SPDX-License-Identifier: MIT
use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub(crate) u64);

impl SubscriptionId {
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Strategy for computing the delay between retry attempts.
///
/// Used by [`FailurePolicy`] to control back-off behaviour when a handler
/// fails and is eligible for retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryStrategy {
    /// Wait a fixed duration between each retry.
    Fixed(Duration),

    /// Exponential back-off: the delay doubles on each attempt, capped at
    /// `max`.
    ///
    /// The delay for attempt *n* (0-indexed) is `min(base * 2^n, max)`.
    Exponential {
        /// Initial delay (attempt 0).
        base: Duration,
        /// Upper bound on the delay.
        max: Duration,
    },

    /// Exponential back-off with random jitter added to each delay.
    ///
    /// The computed delay is `rand(0 ..= min(base * 2^n, max))`.
    ///
    /// Jitter is derived from [`SystemTime`] nanoseconds
    /// — lightweight but **not** cryptographically random.
    ExponentialWithJitter {
        /// Initial delay (attempt 0).
        base: Duration,
        /// Upper bound on the delay before jitter.
        max: Duration,
    },
}

impl RetryStrategy {
    /// Compute the delay for the given zero-based retry attempt.
    pub fn delay_for_attempt(&self, attempt: usize) -> Duration {
        match *self {
            RetryStrategy::Fixed(d) => d,
            RetryStrategy::Exponential { base, max } => {
                let factor = 1u64.checked_shl(attempt as u32).unwrap_or(u64::MAX);
                let delay = base.saturating_mul(factor as u32);
                if delay > max { max } else { delay }
            }
            RetryStrategy::ExponentialWithJitter { base, max } => {
                let factor = 1u64.checked_shl(attempt as u32).unwrap_or(u64::MAX);
                let delay = base.saturating_mul(factor as u32);
                let capped = if delay > max { max } else { delay };
                // Simple jitter: pick a random fraction of the capped delay.
                // We use a lightweight approach without pulling in the `rand` crate.
                let nanos = capped.as_nanos() as u64;
                if nanos == 0 {
                    Duration::ZERO
                } else {
                    // Use the current time's nanoseconds as a cheap entropy source.
                    let entropy = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos() as u64;
                    let jittered = entropy % (nanos + 1);
                    Duration::from_nanos(jittered)
                }
            }
        }
    }
}

/// Policy controlling how handler failures are treated.
///
/// - `max_retries`: how many *additional* attempts after the first failure
///   (0 means no retries). **Only supported for async handlers.** Sync
///   handlers must use [`NoRetryPolicy`] instead; attempting to pass a
///   `FailurePolicy` to a sync handler via
///   [`subscribe_with_policy`](crate::EventBus::subscribe_with_policy) is a
///   compile-time error.
/// - `retry_strategy`: optional [`RetryStrategy`] controlling the delay
///   between retries. When `None`, retries happen immediately. Ignored when
///   `max_retries` is 0. Only applies to async handlers.
/// - `dead_letter`: whether a [`DeadLetter`] event is emitted after all
///   attempts are exhausted (or on first failure for sync handlers).
///   Automatically forced to `false` for dead-letter listeners to prevent
///   infinite recursion.
///
/// All fields are public for convenience; invalid combinations (e.g.
/// `retry_strategy` set with `max_retries: 0`) are harmless but have no effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailurePolicy {
    pub max_retries: usize,
    pub retry_strategy: Option<RetryStrategy>,
    pub dead_letter: bool,
}

impl Default for FailurePolicy {
    fn default() -> Self {
        Self {
            max_retries: 0,
            retry_strategy: None,
            dead_letter: true,
        }
    }
}

impl FailurePolicy {
    pub const fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set a custom [`RetryStrategy`].
    pub fn with_retry_strategy(mut self, strategy: RetryStrategy) -> Self {
        self.retry_strategy = Some(strategy);
        self
    }

    pub const fn with_dead_letter(mut self, dead_letter: bool) -> Self {
        self.dead_letter = dead_letter;
        self
    }
}

/// Failure policy for handlers that do not support retries.
///
/// This type is accepted by [`subscribe_with_policy`](crate::EventBus::subscribe_with_policy)
/// for sync handlers and by [`subscribe_once_with_policy`](crate::EventBus::subscribe_once_with_policy)
/// for all handler types. It contains only the `dead_letter` flag since
/// retry-related fields (`max_retries`, `retry_strategy`) are not applicable.
///
/// Use [`FailurePolicy`] instead when subscribing async handlers that need
/// retry support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoRetryPolicy {
    pub dead_letter: bool,
}

impl Default for NoRetryPolicy {
    fn default() -> Self {
        Self { dead_letter: true }
    }
}

impl NoRetryPolicy {
    pub const fn with_dead_letter(mut self, dead_letter: bool) -> Self {
        self.dead_letter = dead_letter;
        self
    }
}

impl From<NoRetryPolicy> for FailurePolicy {
    fn from(policy: NoRetryPolicy) -> FailurePolicy {
        FailurePolicy {
            max_retries: 0,
            retry_strategy: None,
            dead_letter: policy.dead_letter,
        }
    }
}

// ── IntoFailurePolicy ────────────────────────────────────────────────────────

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::FailurePolicy {}
    impl Sealed for super::NoRetryPolicy {}
}

/// Trait that converts a policy type into a [`FailurePolicy`] suitable for the
/// handler's dispatch mode.
///
/// This trait is **sealed** — it cannot be implemented outside this crate.
///
/// The marker type `M` ([`AsyncMode`](crate::handler::AsyncMode),
/// [`SyncMode`](crate::handler::SyncMode),
/// [`AsyncFnMode`](crate::handler::AsyncFnMode), or
/// [`SyncFnMode`](crate::handler::SyncFnMode)) is inferred from the handler via
/// [`IntoHandler<E, M>`](crate::handler::IntoHandler), so callers never need
/// to specify it explicitly. The type system enforces:
///
/// - **Async handlers** accept both [`FailurePolicy`] (full retry support) and
///   [`NoRetryPolicy`] (dead-letter only, no retries).
/// - **Sync handlers** accept only [`NoRetryPolicy`]. Passing a
///   [`FailurePolicy`] to a sync handler is a compile-time error.
pub trait IntoFailurePolicy<M>: sealed::Sealed {
    /// Convert into the internal [`FailurePolicy`] representation.
    fn into_failure_policy(self) -> FailurePolicy;
}

impl IntoFailurePolicy<crate::handler::AsyncMode> for FailurePolicy {
    fn into_failure_policy(self) -> FailurePolicy {
        self
    }
}

impl IntoFailurePolicy<crate::handler::AsyncFnMode> for FailurePolicy {
    fn into_failure_policy(self) -> FailurePolicy {
        self
    }
}

impl IntoFailurePolicy<crate::handler::AsyncMode> for NoRetryPolicy {
    fn into_failure_policy(self) -> FailurePolicy {
        self.into()
    }
}

impl IntoFailurePolicy<crate::handler::AsyncFnMode> for NoRetryPolicy {
    fn into_failure_policy(self) -> FailurePolicy {
        self.into()
    }
}

impl IntoFailurePolicy<crate::handler::SyncMode> for NoRetryPolicy {
    fn into_failure_policy(self) -> FailurePolicy {
        self.into()
    }
}

impl IntoFailurePolicy<crate::handler::SyncFnMode> for NoRetryPolicy {
    fn into_failure_policy(self) -> FailurePolicy {
        self.into()
    }
}

/// A dead-letter record emitted when a handler exhausts all retry attempts.
///
/// The original error is stored as a [`String`] rather than a typed error
/// because `DeadLetter` must be `Clone` (it is published as an event) and
/// `Box<dyn Error>` does not implement `Clone`. Use [`error`](Self::error)
/// for diagnostics or pattern-match on the stringified message.
///
/// The original event payload is available via [`event`](Self::event) as a
/// type-erased `Arc`. Consumers can call
/// `dead_letter.event.downcast_ref::<OriginalEvent>()` to inspect the
/// payload that caused the failure.
#[derive(Clone, Debug)]
pub struct DeadLetter {
    /// The `type_name` of the event that failed.
    pub event_name: &'static str,
    /// The subscription that failed to handle the event.
    pub subscription_id: SubscriptionId,
    /// Total number of attempts (initial + retries).
    pub attempts: usize,
    /// Stringified error from the last failed attempt.
    pub error: String,
    /// The original event payload, type-erased.
    ///
    /// Use `downcast_ref::<E>()` to recover the concrete event type.
    pub event: Arc<dyn Any + Send + Sync>,
    /// When the terminal failure occurred.
    pub failed_at: SystemTime,
    /// Human-readable name of the listener that failed, if provided.
    pub listener_name: Option<&'static str>,
}

pub trait Event: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Event for T {}

/// Information about a single registered listener, as reported by
/// [`BusStats`].
#[derive(Debug, Clone)]
pub struct ListenerInfo {
    /// The unique subscription identifier.
    pub subscription_id: SubscriptionId,
    /// Human-readable name, if the handler provides one.
    pub name: Option<&'static str>,
}

/// A point-in-time snapshot of the event bus internal state.
///
/// Obtained via [`EventBus::stats()`](crate::EventBus::stats).
#[derive(Debug, Clone)]
pub struct BusStats {
    /// Total number of active subscriptions across all event types.
    pub total_subscriptions: usize,
    /// Per-event-type listener details, keyed by the event type name.
    pub subscriptions_by_event: HashMap<&'static str, Vec<ListenerInfo>>,
    /// The type names of all event types that currently have at least one
    /// registered listener.
    pub registered_event_types: Vec<&'static str>,
    /// The configured channel buffer capacity.
    pub queue_capacity: usize,
    /// Number of async handler tasks currently in flight.
    pub in_flight_async: usize,
    /// Whether [`EventBus::shutdown`](crate::EventBus::shutdown) has been called.
    pub shutdown_called: bool,
}

/// Internal configuration for the event bus actor.
#[derive(Debug, Clone)]
pub(crate) struct BusConfig {
    pub buffer_size: usize,
    pub handler_timeout: Option<Duration>,
    pub max_concurrent_async: Option<usize>,
    pub default_failure_policy: FailurePolicy,
    pub shutdown_timeout: Option<Duration>,
}

impl Default for BusConfig {
    fn default() -> Self {
        Self {
            buffer_size: 256,
            handler_timeout: None,
            max_concurrent_async: None,
            default_failure_policy: FailurePolicy::default(),
            shutdown_timeout: None,
        }
    }
}
