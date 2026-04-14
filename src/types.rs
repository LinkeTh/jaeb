use std::any::Any;
use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
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
/// Used by [`AsyncSubscriptionPolicy`] to control back-off behaviour when a
/// handler fails and is eligible for retry.
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
                // We use `RandomState` (per-process random seed) hashed with a
                // counter to get decent distribution without pulling in `rand`.
                let nanos = capped.as_nanos() as u64;
                if nanos == 0 {
                    Duration::ZERO
                } else {
                    use std::sync::atomic::{AtomicU64, Ordering};
                    static COUNTER: AtomicU64 = AtomicU64::new(0);
                    let tick = COUNTER.fetch_add(1, Ordering::Relaxed);
                    let mut hasher = RandomState::new().build_hasher();
                    hasher.write_u64(tick);
                    let entropy = hasher.finish();
                    let jittered = entropy % (nanos + 1);
                    Duration::from_nanos(jittered)
                }
            }
        }
    }
}

/// Subscription policy for asynchronous handlers.
///
/// Async handlers can configure retry behaviour and dead-letter emission.
/// Ordering is always FIFO by registration order within the async lane, so no
/// priority field is exposed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsyncSubscriptionPolicy {
    /// Number of additional attempts after the first failure (0 = no retries).
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

impl Default for AsyncSubscriptionPolicy {
    fn default() -> Self {
        Self {
            max_retries: 0,
            retry_strategy: None,
            dead_letter: true,
        }
    }
}

impl AsyncSubscriptionPolicy {
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

/// Subscription policy for synchronous handlers.
///
/// Sync handlers execute exactly once per publish and may provide a priority
/// hint to control ordering within the sync dispatch lane.
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

/// Default subscription policies applied by [`EventBus::subscribe`](crate::EventBus::subscribe)
///
/// `policy` is the default for async subscriptions, while `sync_policy` is the
/// default for sync and one-shot subscriptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubscriptionDefaults {
    /// Default policy applied by [`EventBus::subscribe`](crate::EventBus::subscribe)
    /// for async handlers.
    pub policy: AsyncSubscriptionPolicy,
    /// Default policy applied by [`EventBus::subscribe`](crate::EventBus::subscribe)
    /// for sync handlers.
    pub sync_policy: SyncSubscriptionPolicy,
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

/// Marker trait for all publishable event types.
///
/// Any type that is `Send + Sync + 'static` automatically implements `Event`
/// via a blanket implementation, so no manual implementation is required.
/// To publish an event through the bus APIs, the event type must also be
/// `Clone`.
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
    /// Number of accepted publish operations currently in progress.
    pub dispatches_in_flight: usize,
    /// Number of async listener invocations currently queued or running.
    pub in_flight_async: usize,
    /// Whether [`EventBus::shutdown`](crate::EventBus::shutdown) has been called.
    pub shutdown_called: bool,
}

/// Internal configuration for the event bus runtime.
#[derive(Debug, Clone, Default)]
pub(crate) struct BusConfig {
    pub handler_timeout: Option<Duration>,
    pub max_concurrent_async: Option<usize>,
    pub subscription_defaults: SubscriptionDefaults,
    pub shutdown_timeout: Option<Duration>,
}
