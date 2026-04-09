// SPDX-License-Identifier: MIT
use std::any::Any;
use std::future::Future;
use std::sync::Arc;

use tracing::warn;

use crate::actor::{ErasedAsyncHandler, ErasedHandler, ErasedSyncHandler, EventType};
use crate::error::HandlerResult;
use crate::types::Event;

/// Trait for async struct-based event handlers that hold their own dependencies.
///
/// Implementors can use `async fn` directly without `async-trait`.
///
/// # `Clone` requirement
///
/// The event type `E` must implement `Clone` because async handlers may
/// outlive the dispatch call.  The event is stored behind an `Arc` and each
/// async invocation clones the inner `E` so it can be moved into the spawned
/// future.  The `Arc` clone is O(1); the `E::clone()` cost depends on the
/// event payload.
pub trait EventHandler<E: Event + Clone>: Send + Sync + 'static {
    fn handle(&self, event: &E) -> impl Future<Output = HandlerResult> + Send;

    /// Optional human-readable name for this handler.
    ///
    /// When provided, the name is included in tracing spans, dead-letter
    /// records, and metrics labels. Defaults to `None`.
    fn name(&self) -> Option<&'static str> {
        None
    }
}

/// Trait for synchronous struct-based event handlers.
///
/// Sync handlers receive `&E` directly (via downcast from `&dyn Any`), so
/// there is **no per-invocation allocation** for the event.  The handler
/// runs inline on the actor task (with panic isolation via `catch_unwind`)
/// and `publish()` blocks until it returns, preserving strong ordering
/// guarantees.
///
/// **Sync handlers execute exactly once** — retries are not supported.
/// [`subscribe_with_policy`](crate::EventBus::subscribe_with_policy) enforces
/// this at compile time by requiring [`NoRetryPolicy`](crate::NoRetryPolicy)
/// for sync handlers. On failure, a [`DeadLetter`](crate::DeadLetter) is
/// emitted when enabled.
pub trait SyncEventHandler<E: Event>: Send + Sync + 'static {
    fn handle(&self, event: &E) -> HandlerResult;

    /// Optional human-readable name for this handler.
    ///
    /// When provided, the name is included in tracing spans, dead-letter
    /// records, and metrics labels. Defaults to `None`.
    fn name(&self) -> Option<&'static str> {
        None
    }
}

/// Marker type selecting the asynchronous dispatch path.
pub struct AsyncMode;

/// Marker type selecting the synchronous dispatch path.
pub struct SyncMode;

/// A type-erased handler produced by [`IntoHandler`].
pub(crate) struct RegisteredHandler {
    pub erased: ErasedHandler,
    /// Human-readable name extracted from the handler trait.
    pub name: Option<&'static str>,
}

/// Trait that converts a concrete handler into an erased handler with its dispatch mode.
///
/// This trait is implemented automatically for any type that implements
/// [`EventHandler<E>`] (async) or [`SyncEventHandler<E>`] (sync).
/// You do not need to implement it manually.
///
/// The `Mode` type parameter (`AsyncMode` or `SyncMode`) is inferred from the
/// handler trait, so callers simply write `bus.subscribe(handler)`.
///
/// # Allocation at registration time
///
/// `into_handler()` wraps the handler in `Arc<H>` and creates one closure
/// (either `ErasedAsyncHandler` or `ErasedSyncHandler`) that captures the
/// `Arc`.  This is a **one-time allocation per subscription**, not per
/// dispatch.
#[allow(private_interfaces)]
pub trait IntoHandler<E: Event, Mode> {
    #[doc(hidden)]
    fn into_handler(self) -> RegisteredHandler;
}

#[allow(private_interfaces)]
impl<E, H> IntoHandler<E, AsyncMode> for H
where
    E: Event + Clone,
    H: EventHandler<E>,
{
    fn into_handler(self) -> RegisteredHandler {
        let name = self.name();
        let handler = Arc::new(self);
        let erased: ErasedAsyncHandler = Arc::new(move |any: EventType| {
            let handler = Arc::clone(&handler);
            if let Some(event) = any.as_ref().downcast_ref::<E>() {
                let event = event.clone();
                Box::pin(async move { handler.handle(&event).await }) as _
            } else {
                warn!(expected = std::any::type_name::<E>(), "handler.downcast_failed");
                Box::pin(async { Err("internal error: event type downcast failed".into()) }) as _
            }
        });
        RegisteredHandler {
            erased: ErasedHandler::Async(erased),
            name,
        }
    }
}

#[allow(private_interfaces)]
impl<E, H> IntoHandler<E, SyncMode> for H
where
    E: Event,
    H: SyncEventHandler<E>,
{
    fn into_handler(self) -> RegisteredHandler {
        let name = self.name();
        let handler = Arc::new(self);
        let erased: ErasedSyncHandler = Arc::new(move |any: &(dyn Any + Send + Sync)| {
            if let Some(event) = any.downcast_ref::<E>() {
                handler.handle(event)
            } else {
                warn!(expected = std::any::type_name::<E>(), "handler.downcast_failed");
                Err("internal error: event type downcast failed".into())
            }
        });
        RegisteredHandler {
            erased: ErasedHandler::Sync(erased),
            name,
        }
    }
}
