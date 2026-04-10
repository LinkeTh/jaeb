#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

pub(crate) mod bus;
pub(crate) mod error;
pub(crate) mod handler;
#[cfg(feature = "metrics")]
mod metrics;
pub(crate) mod middleware;
pub(crate) mod registry;
pub(crate) mod subscription;
#[cfg(feature = "test-utils")]
pub(crate) mod test_utils;
pub(crate) mod types;

pub use bus::{EventBus, EventBusBuilder};
pub use error::{ConfigError, EventBusError, HandlerError, HandlerResult};
pub use handler::{AsyncFnMode, AsyncMode, EventHandler, IntoHandler, SyncEventHandler, SyncFnMode, SyncMode};
pub use middleware::{Middleware, MiddlewareDecision, SyncMiddleware, TypedMiddleware, TypedSyncMiddleware};
pub use subscription::{Subscription, SubscriptionGuard};
#[cfg(feature = "test-utils")]
pub use test_utils::{TestBus, TestBusBuilder};
pub use types::{BusStats, DeadLetter, Event, FailurePolicy, IntoFailurePolicy, ListenerInfo, NoRetryPolicy, RetryStrategy, SubscriptionId};
