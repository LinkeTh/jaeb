// SPDX-License-Identifier: MIT
#![forbid(unsafe_code)]

//! JAEB is an in-process, actor-based event bus for Tokio applications.
//!
//! It supports synchronous and asynchronous listeners, explicit dependency
//! injection via handler structs, unsubscribe handles, retry/dead-letter
//! failure policies, and graceful shutdown.
//!
//! # Requirements
//!
//! An active **Tokio runtime** must be available when constructing an
//! [`EventBus`] (via [`new`](EventBus::new) or
//! [`builder().build()`](EventBusBuilder::build)), because the internal
//! actor task is spawned immediately via [`tokio::spawn`]. Both construction
//! methods return [`Result`] and validate their configuration before spawning.
//!
//! # Important semantics
//!
//! - **Sync handlers** — executed inline on the actor task (with panic
//!   isolation via `catch_unwind`) and complete before
//!   [`EventBus::publish`](EventBus::publish) returns. Sync handlers
//!   execute exactly once — retries are not supported.
//! - **Async handlers** — spawned into a [`JoinSet`](tokio::task::JoinSet);
//!   `publish` may return before they finish.  Async events require `E: Clone`
//!   because the event is cloned for each handler invocation. Retries and
//!   delays are supported via [`FailurePolicy`].
//! - **Shutdown** — [`EventBus::shutdown`](EventBus::shutdown) drains
//!   queued messages, then waits for (or aborts) in-flight async tasks.
//!   Shutdown is idempotent: subsequent calls return `Ok(())`.
//! - **Dead letters** — after all retries are exhausted a
//!   [`DeadLetter`] is published unless disabled.
//!   A failing dead-letter handler will **not** produce another dead letter
//!   (recursion is guarded).

mod actor;
pub mod bus;
pub mod error;
pub mod handler;
#[cfg(feature = "metrics")]
mod metrics;
pub mod middleware;
pub mod subscription;
#[cfg(feature = "test-utils")]
pub mod test_utils;
pub mod types;

pub use bus::{EventBus, EventBusBuilder};
pub use error::{ConfigError, EventBusError, HandlerError, HandlerResult};
pub use handler::{AsyncMode, EventHandler, IntoHandler, SyncEventHandler, SyncMode};
pub use middleware::{Middleware, MiddlewareDecision, SyncMiddleware};
pub use subscription::{Subscription, SubscriptionGuard};
#[cfg(feature = "test-utils")]
pub use test_utils::{TestBus, TestBusBuilder};
pub use types::{BusStats, DeadLetter, Event, FailurePolicy, IntoFailurePolicy, ListenerInfo, NoRetryPolicy, RetryStrategy, SubscriptionId};
