//! Middleware traits for cross-cutting event interception.
//!
//! Middlewares run **before** listener dispatch and can inspect (or reject) any
//! event flowing through the bus. They receive the event as `&dyn Any`, so a
//! single middleware can handle multiple event types via downcasting.
//!
//! # Ordering
//!
//! Middlewares execute in FIFO registration order. The first middleware to
//! return [`MiddlewareDecision::Reject`] short-circuits the pipeline — no
//! further middlewares run and no listeners are invoked.

use std::any::Any;
use std::future::Future;

use crate::types::Event;

/// Decision returned by a middleware after inspecting an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiddlewareDecision {
    /// Allow the event to proceed to listeners (and the next middleware).
    Continue,
    /// Reject the event with a reason string. No listeners will fire.
    Reject(String),
}

/// Async middleware trait.
///
/// Implement this for middleware that needs to perform async work (e.g., check
/// an external service, acquire a lock, etc.).
pub trait Middleware: Send + Sync + 'static {
    /// Inspect the event and decide whether to continue or reject.
    ///
    /// `event_name` is the `std::any::type_name` of the concrete event type.
    fn process<'a>(&'a self, event_name: &'static str, event: &'a (dyn Any + Send + Sync)) -> impl Future<Output = MiddlewareDecision> + Send + 'a;
}

/// Sync middleware trait.
///
/// Implement this for lightweight, non-blocking middleware that can make a
/// decision synchronously.
pub trait SyncMiddleware: Send + Sync + 'static {
    /// Inspect the event and decide whether to continue or reject.
    fn process(&self, event_name: &'static str, event: &(dyn Any + Send + Sync)) -> MiddlewareDecision;
}

/// Async middleware scoped to a specific event type `E`.
///
/// This middleware runs only when publishing events of type `E`.
pub trait TypedMiddleware<E: Event>: Send + Sync + 'static {
    /// Inspect a typed event and decide whether to continue or reject.
    fn process<'a>(&'a self, event_name: &'static str, event: &'a E) -> impl Future<Output = MiddlewareDecision> + Send + 'a;
}

/// Sync middleware scoped to a specific event type `E`.
///
/// This middleware runs only when publishing events of type `E`.
pub trait TypedSyncMiddleware<E: Event>: Send + Sync + 'static {
    /// Inspect a typed event and decide whether to continue or reject.
    fn process(&self, event_name: &'static str, event: &E) -> MiddlewareDecision;
}
