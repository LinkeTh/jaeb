use std::future::Future;

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
