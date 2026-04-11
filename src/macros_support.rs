use core::future::Future;
use core::pin::Pin;

use crate::{EventBus, EventBusError, Subscription};

/// Trait implemented by macro-generated handler registrar types.
pub trait HandlerRegistrar: Send + Sync + 'static {
    /// Subscribe this handler to the provided event bus.
    fn register<'a>(&self, bus: &'a EventBus) -> Pin<Box<dyn Future<Output = Result<Subscription, EventBusError>> + Send + 'a>>;
}

inventory::collect!(&'static dyn HandlerRegistrar);

/// Registers all handlers discovered via inventory auto-discovery.
pub async fn register_all(bus: &EventBus) -> Result<(), EventBusError> {
    for registrar in inventory::iter::<&'static dyn HandlerRegistrar>.into_iter() {
        let _ = registrar.register(bus).await?;
    }

    Ok(())
}

/// Re-exports used by macro-generated code.
#[doc(hidden)]
pub mod _private {
    pub use inventory;
}
