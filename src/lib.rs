// SPDX-License-Identifier: MIT
#![forbid(unsafe_code)]

//! JAEB is an in-process, actor-based event bus for Tokio applications.
//!
//! It supports synchronous and asynchronous listeners, explicit dependency
//! injection via handler structs, unsubscribe handles, retry/dead-letter
//! failure policies, and graceful shutdown.

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
