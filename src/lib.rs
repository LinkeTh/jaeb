#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

pub(crate) mod bus;
pub(crate) mod error;
pub(crate) mod handler;
#[cfg(feature = "macros")]
#[doc(hidden)]
pub mod macros_support;
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
#[cfg(feature = "macros")]
pub use jaeb_macros::{handler, register_handlers};
pub use middleware::{Middleware, MiddlewareDecision, SyncMiddleware, TypedMiddleware, TypedSyncMiddleware};
pub use subscription::{Subscription, SubscriptionGuard};
#[cfg(feature = "test-utils")]
pub use test_utils::{TestBus, TestBusBuilder};
pub use types::{
    BusStats, DeadLetter, Event, IntoSubscriptionPolicy, ListenerInfo, RetryStrategy, SubscriptionId, SubscriptionPolicy, SyncSubscriptionPolicy,
};
#[allow(deprecated)]
pub use types::{FailurePolicy, IntoFailurePolicy, NoRetryPolicy};
