use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Unique identifier for a listener or middleware registration.
///
/// Assigned by the bus at subscription time and remains stable for the
/// lifetime of the registration. Obtain it from
/// [`Subscription::id`](crate::Subscription::id) or
/// [`SubscriptionGuard::id`](crate::SubscriptionGuard::id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub(crate) u64);

impl SubscriptionId {
    /// Return the raw numeric value of the identifier.
    ///
    /// Useful for logging and tracing; do not rely on the magnitude or
    /// sequence of values.
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
/// Used by [`SubscriptionPolicy`] to control back-off behaviour when a handler
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
                let factor = u32::try_from(factor).unwrap_or(u32::MAX);
                let delay = base.saturating_mul(factor);
                if delay > max { max } else { delay }
            }
            RetryStrategy::ExponentialWithJitter { base, max } => {
                let factor = 1u64.checked_shl(attempt as u32).unwrap_or(u64::MAX);
                let factor = u32::try_from(factor).unwrap_or(u32::MAX);
                let delay = base.saturating_mul(factor);
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

/// Policy controlling how a subscription is scheduled and how failures are treated.
///
/// - `priority`: listener ordering hint. Higher values are dispatched first
///   within the same dispatch lane (sync or async). Equal priorities keep
///   FIFO registration order.
/// - `max_retries`: how many *additional* attempts after the first failure
///   (0 means no retries). **Only supported for async handlers.** Sync
///   handlers must use [`SyncSubscriptionPolicy`] instead; attempting to pass a
///   `SubscriptionPolicy` to a sync handler via
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
pub struct SubscriptionPolicy {
    /// Listener ordering hint (higher runs first).
    ///
    /// Applied independently within sync and async lanes.
    pub priority: i32,
    /// Number of additional attempts after the first failure (0 = no retries).
    ///
    /// Only honoured for async handlers. Sync handlers always behave as if
    /// this is `0`.
    pub max_retries: usize,
    /// Optional back-off strategy between retry attempts.
    ///
    /// `None` means retry immediately. Ignored when `max_retries` is `0`.
    pub retry_strategy: Option<RetryStrategy>,
    /// Emit a [`DeadLetter`] event after all attempts are exhausted.
    ///
    /// Automatically forced to `false` for dead-letter listeners.
    pub dead_letter: bool,
}

impl Default for SubscriptionPolicy {
    fn default() -> Self {
        Self {
            priority: 0,
            max_retries: 0,
            retry_strategy: None,
            dead_letter: true,
        }
    }
}

impl SubscriptionPolicy {
    /// Set listener priority (builder-style).
    ///
    /// Higher values are dispatched first within each lane.
    pub const fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Set the maximum number of retries (builder-style).
    ///
    /// `0` disables retries. Only applicable to async handlers.
    pub const fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set a custom [`RetryStrategy`].
    pub fn with_retry_strategy(mut self, strategy: RetryStrategy) -> Self {
        self.retry_strategy = Some(strategy);
        self
    }

    /// Enable or disable dead-letter emission for this policy (builder-style).
    pub const fn with_dead_letter(mut self, dead_letter: bool) -> Self {
        self.dead_letter = dead_letter;
        self
    }
}

/// Subscription policy for handlers that do not support retries.
///
/// This type is accepted by [`subscribe_with_policy`](crate::EventBus::subscribe_with_policy)
/// for sync handlers and by [`subscribe_once_with_policy`](crate::EventBus::subscribe_once_with_policy)
/// for all handler types. It contains only the `dead_letter` flag since
/// retry-related fields (`max_retries`, `retry_strategy`) are not applicable.
///
/// Use [`SubscriptionPolicy`] instead when subscribing async handlers that need
/// retry support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncSubscriptionPolicy {
    /// Listener ordering hint (higher runs first).
    pub priority: i32,
    /// Emit a [`DeadLetter`] event on failure.
    ///
    /// Automatically forced to `false` for dead-letter listeners.
    pub dead_letter: bool,
}

impl Default for SyncSubscriptionPolicy {
    fn default() -> Self {
        Self {
            priority: 0,
            dead_letter: true,
        }
    }
}

impl SyncSubscriptionPolicy {
    /// Set listener priority (builder-style).
    pub const fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Enable or disable dead-letter emission for this policy (builder-style).
    pub const fn with_dead_letter(mut self, dead_letter: bool) -> Self {
        self.dead_letter = dead_letter;
        self
    }
}

impl From<SyncSubscriptionPolicy> for SubscriptionPolicy {
    fn from(policy: SyncSubscriptionPolicy) -> SubscriptionPolicy {
        SubscriptionPolicy {
            priority: policy.priority,
            max_retries: 0,
            retry_strategy: None,
            dead_letter: policy.dead_letter,
        }
    }
}

// ── IntoSubscriptionPolicy ───────────────────────────────────────────────────

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::SubscriptionPolicy {}
    impl Sealed for super::SyncSubscriptionPolicy {}
}

/// Trait that converts a policy type into a [`SubscriptionPolicy`] suitable for
/// the
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
/// - **Async handlers** accept both [`SubscriptionPolicy`] (full retry
///   support) and [`SyncSubscriptionPolicy`] (dead-letter only, no retries).
/// - **Sync handlers** accept only [`SyncSubscriptionPolicy`]. Passing a
///   [`SubscriptionPolicy`] to a sync handler is a compile-time error.
pub trait IntoSubscriptionPolicy<M>: sealed::Sealed {
    /// Convert into the internal [`SubscriptionPolicy`] representation.
    fn into_subscription_policy(self) -> SubscriptionPolicy;
}

#[allow(dead_code)]
#[deprecated(since = "0.3.3", note = "renamed to SubscriptionPolicy")]
pub type FailurePolicy = SubscriptionPolicy;

#[allow(dead_code)]
#[deprecated(since = "0.3.3", note = "renamed to SyncSubscriptionPolicy")]
pub type NoRetryPolicy = SyncSubscriptionPolicy;

#[allow(unused_imports)]
#[deprecated(since = "0.3.3", note = "renamed to IntoSubscriptionPolicy")]
pub use IntoSubscriptionPolicy as IntoFailurePolicy;

impl IntoSubscriptionPolicy<crate::handler::AsyncMode> for SubscriptionPolicy {
    fn into_subscription_policy(self) -> SubscriptionPolicy {
        self
    }
}

impl IntoSubscriptionPolicy<crate::handler::AsyncFnMode> for SubscriptionPolicy {
    fn into_subscription_policy(self) -> SubscriptionPolicy {
        self
    }
}

impl IntoSubscriptionPolicy<crate::handler::AsyncMode> for SyncSubscriptionPolicy {
    fn into_subscription_policy(self) -> SubscriptionPolicy {
        self.into()
    }
}

impl IntoSubscriptionPolicy<crate::handler::AsyncFnMode> for SyncSubscriptionPolicy {
    fn into_subscription_policy(self) -> SubscriptionPolicy {
        self.into()
    }
}

impl IntoSubscriptionPolicy<crate::handler::SyncMode> for SyncSubscriptionPolicy {
    fn into_subscription_policy(self) -> SubscriptionPolicy {
        self.into()
    }
}

impl IntoSubscriptionPolicy<crate::handler::SyncFnMode> for SyncSubscriptionPolicy {
    fn into_subscription_policy(self) -> SubscriptionPolicy {
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
    /// Human-readable name of the handler that failed, if provided.
    pub handler_name: Option<&'static str>,
}

impl DeadLetter {
    /// Deprecated accessor for `handler_name`.
    #[deprecated(since = "0.3.6", note = "renamed to handler_name")]
    pub fn listener_name(&self) -> Option<&'static str> {
        self.handler_name
    }
}

/// Marker trait for all publishable event types.
///
/// Any type that is `Send + Sync + 'static` automatically implements `Event`
/// via a blanket implementation, so no manual implementation is required.
/// Async handlers additionally require `E: Clone`.
///
/// # Examples
///
/// ```
/// // No derive or impl needed — the blanket impl does it automatically.
/// #[derive(Clone)]
/// struct OrderPlaced { pub order_id: u64 }
/// // OrderPlaced: Event is satisfied automatically.
/// ```
pub trait Event: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Event for T {}

/// Information about a single registered handler, as reported by
/// [`BusStats`].
#[derive(Debug, Clone)]
pub struct HandlerInfo {
    /// The unique subscription identifier.
    pub subscription_id: SubscriptionId,
    /// Human-readable name, if the handler provides one.
    pub name: Option<&'static str>,
}

#[allow(dead_code)]
#[deprecated(since = "0.3.6", note = "renamed to HandlerInfo")]
pub type ListenerInfo = HandlerInfo;

/// A point-in-time snapshot of the event bus internal state.
///
/// Obtained via [`EventBus::stats()`](crate::EventBus::stats).
#[derive(Debug, Clone)]
pub struct BusStats {
    /// Total number of active subscriptions across all event types.
    pub total_subscriptions: usize,
    /// Per-event-type listener details, keyed by the event type name.
    pub subscriptions_by_event: HashMap<&'static str, Vec<HandlerInfo>>,
    /// The type names of all event types that currently have at least one
    /// registered listener.
    pub registered_event_types: Vec<&'static str>,
    /// The configured channel buffer capacity.
    pub queue_capacity: usize,
    /// Number of currently available publish permits in the internal semaphore.
    pub publish_permits_available: usize,
    /// Number of currently occupied publish permits.
    pub publish_in_flight: usize,
    /// Number of async handler tasks currently in flight.
    pub in_flight_async: usize,
    /// Whether [`EventBus::shutdown`](crate::EventBus::shutdown) has been called.
    pub shutdown_called: bool,
}

/// Internal configuration for the event bus runtime.
#[derive(Debug, Clone)]
pub(crate) struct BusConfig {
    pub buffer_size: usize,
    pub handler_timeout: Option<Duration>,
    pub max_concurrent_async: Option<usize>,
    pub default_subscription_policy: SubscriptionPolicy,
    pub shutdown_timeout: Option<Duration>,
}

impl Default for BusConfig {
    fn default() -> Self {
        Self {
            buffer_size: 256,
            handler_timeout: None,
            max_concurrent_async: None,
            default_subscription_policy: SubscriptionPolicy::default(),
            shutdown_timeout: None,
        }
    }
}
