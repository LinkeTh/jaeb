// SPDX-License-Identifier: MIT
#![forbid(unsafe_code)]

//! JAEB is an in-process, actor-based event bus for Tokio applications.
//!
//! It supports synchronous and asynchronous listeners, explicit dependency
//! injection via handler structs, unsubscribe handles, retry/dead-letter
//! failure policies, and graceful shutdown.
//!
//! # Important semantics
//!
//! - **Sync handlers** — dispatched via `tokio::spawn` and awaited before
//!   [`EventBus::publish`](bus::EventBus::publish) returns.
//! - **Async handlers** — spawned into a [`JoinSet`](tokio::task::JoinSet);
//!   `publish` may return before they finish.  Async events require `E: Clone`
//!   because the event is cloned for each handler invocation.
//! - **Shutdown** — [`EventBus::shutdown`](bus::EventBus::shutdown) drains
//!   queued messages, then waits for (or aborts) in-flight async tasks.
//!   Calling `shutdown` again returns [`EventBusError::ActorStopped`].
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
pub mod subscription;
pub mod types;

pub use bus::{EventBus, EventBusBuilder};
pub use error::{EventBusError, HandlerError, HandlerResult};
pub use handler::{AsyncMode, EventHandler, IntoHandler, SyncEventHandler, SyncMode};
pub use subscription::Subscription;
pub use types::{DeadLetter, Event, FailurePolicy, SubscriptionId};
