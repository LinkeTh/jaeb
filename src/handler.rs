// SPDX-License-Identifier: MIT
use std::future::Future;
use std::sync::Arc;

use tracing::warn;

use crate::actor::{ArcEvent, ErasedHandler, ListenerMode};
use crate::error::HandlerResult;
use crate::types::Event;

/// Trait for async struct-based event handlers that hold their own dependencies.
///
/// Implementors can use `async fn` directly without `async-trait`.
pub trait EventHandler<E: Event + Clone>: Send + Sync + 'static {
    fn handle(&self, event: &E) -> impl Future<Output = HandlerResult> + Send;
}

/// Trait for synchronous struct-based event handlers.
pub trait SyncEventHandler<E: Event>: Send + Sync + 'static {
    fn handle(&self, event: &E) -> HandlerResult;
}

/// Marker type selecting the asynchronous dispatch path.
pub struct AsyncMode;

/// Marker type selecting the synchronous dispatch path.
pub struct SyncMode;

/// A type-erased handler paired with its dispatch mode, produced by [`IntoHandler`].
pub(crate) struct RegisteredHandler {
    pub erased: ErasedHandler,
    pub mode: ListenerMode,
}

/// Trait that converts a concrete handler into an erased handler with its dispatch mode.
///
/// This trait is implemented automatically for any type that implements
/// [`EventHandler<E>`] (async) or [`SyncEventHandler<E>`] (sync).
/// You do not need to implement it manually.
///
/// The `Mode` type parameter (`AsyncMode` or `SyncMode`) is inferred from the
/// handler trait, so callers simply write `bus.subscribe(handler)`.
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
        let handler = Arc::new(self);
        let erased: ErasedHandler = Arc::new(move |any: ArcEvent| {
            let handler = Arc::clone(&handler);
            if let Some(event) = any.as_ref().downcast_ref::<E>() {
                let event = event.clone();
                Box::pin(async move { handler.handle(&event).await })
            } else {
                warn!(expected = std::any::type_name::<E>(), "handler.downcast_failed");
                Box::pin(async { Err("internal error: event type downcast failed".into()) })
            }
        });
        RegisteredHandler {
            erased,
            mode: ListenerMode::Async,
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
        let handler = Arc::new(self);
        let erased: ErasedHandler = Arc::new(move |any: ArcEvent| {
            let result = if let Some(event) = any.as_ref().downcast_ref::<E>() {
                handler.handle(event)
            } else {
                warn!(expected = std::any::type_name::<E>(), "handler.downcast_failed");
                Err("internal error: event type downcast failed".into())
            };
            Box::pin(async move { result })
        });
        RegisteredHandler {
            erased,
            mode: ListenerMode::Sync,
        }
    }
}
