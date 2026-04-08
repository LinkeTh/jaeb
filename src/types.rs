// SPDX-License-Identifier: MIT
use std::fmt;
use std::time::Duration;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailurePolicy {
    pub max_retries: usize,
    pub retry_delay: Option<Duration>,
    pub dead_letter: bool,
}

impl Default for FailurePolicy {
    fn default() -> Self {
        Self {
            max_retries: 0,
            retry_delay: None,
            dead_letter: true,
        }
    }
}

impl FailurePolicy {
    pub const fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_retry_delay(mut self, retry_delay: Duration) -> Self {
        self.retry_delay = Some(retry_delay);
        self
    }

    pub const fn with_dead_letter(mut self, dead_letter: bool) -> Self {
        self.dead_letter = dead_letter;
        self
    }
}

#[derive(Clone, Debug)]
pub struct DeadLetter {
    pub event_name: &'static str,
    pub subscription_id: SubscriptionId,
    pub attempts: usize,
    pub error: String,
}

pub trait Event: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Event for T {}

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
