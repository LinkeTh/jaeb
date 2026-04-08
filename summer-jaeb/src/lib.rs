// SPDX-License-Identifier: MIT
mod config;

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use jaeb::EventBus;
use summer::app::AppBuilder;
use summer::async_trait;
use summer::config::ConfigRegistry;
use summer::plugin::{MutableComponentRegistry, Plugin};
use tracing::info;

use crate::config::Config;

pub use crate::config::Config as JaebConfig;

// Re-export the proc macro so users write `use summer_jaeb::event_listener;`
pub use summer_jaeb_macros::event_listener;

// ── Auto-registration infrastructure ─────────────────────────────────────────

/// Trait implemented by macro-generated registrar structs.
///
/// Each `#[event_listener]` invocation generates a type that implements this trait.
/// The `SummerJaeb` plugin collects all implementations via `inventory` and calls
/// `register()` during startup.
///
/// Uses `AppBuilder` directly because summer's `ComponentRegistry` trait is not
/// dyn-compatible (it has generic methods like `get_component<T>()`).
pub trait TypedListenerRegistrar: Send + Sync + 'static {
    /// Subscribe this listener to the event bus, resolving any required state
    /// from the application's component registry.
    fn register<'a>(&self, bus: &'a EventBus, app: &'a AppBuilder) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

// Collect `&'static dyn TypedListenerRegistrar` — matches summer-rs's own pattern
// for `inventory::collect!(&'static dyn Plugin)`.
inventory::collect!(&'static dyn TypedListenerRegistrar);

/// Iterate all auto-registered listener registrars discovered via `inventory`.
///
/// This is an internal helper used by `SummerJaeb::build()`. The double reference
/// (`&'static dyn`) comes from how `inventory` stores and iterates trait objects.
#[must_use]
pub(crate) fn auto_listeners() -> impl Iterator<Item = &'static &'static dyn TypedListenerRegistrar> {
    inventory::iter::<&'static dyn TypedListenerRegistrar>.into_iter()
}

/// Re-exports used by macro-generated code.
///
/// This module is **not** part of the public API — it exists solely so that
/// `#[event_listener]` expansion can reference `inventory` without requiring
/// downstream crates to add it as a direct dependency.
#[doc(hidden)]
pub mod _private {
    pub use inventory;
}

// ── Plugin ───────────────────────────────────────────────────────────────────

/// summer-rs plugin that registers a [`jaeb::EventBus`] as an application component
/// and auto-subscribes all `#[event_listener]` functions.
///
/// # Usage
///
/// ```toml
/// # Cargo.toml
/// [dependencies]
/// summer-jaeb = "0.1"
/// # Optional: enable handler timing metrics
/// # summer-jaeb = { version = "0.1", features = ["metrics"] }
/// ```
///
/// ```rust,ignore
/// use summer::app::App;
/// use summer_jaeb::SummerJaeb;
///
/// #[tokio::main]
/// async fn main() {
///     App::new()
///         .add_plugin(SummerJaeb)
///         .run()
///         .await;
/// }
/// ```
///
/// Configuration (`app.toml`):
/// ```toml
/// [jaeb]
/// buffer_size = 512
/// handler_timeout_secs = 5
/// max_concurrent_async = 100
/// shutdown_timeout_secs = 10
/// ```
///
/// Retrieve the bus elsewhere via dependency injection:
/// ```rust,ignore
/// use jaeb::EventBus;
/// use summer::plugin::ComponentRegistry;
///
/// let bus: EventBus = app.get_component().unwrap();
/// ```
pub struct SummerJaeb;

#[async_trait]
impl Plugin for SummerJaeb {
    fn name(&self) -> &str {
        "SummerJaeb"
    }

    async fn build(&self, app: &mut AppBuilder) {
        let config = app.get_config::<Config>().expect("summer-jaeb: failed to load [jaeb] config");

        let mut builder = EventBus::builder();

        if let Some(size) = config.buffer_size {
            builder = builder.buffer_size(size);
        }
        if let Some(secs) = config.handler_timeout_secs {
            builder = builder.handler_timeout(Duration::from_secs(secs));
        }
        if let Some(max) = config.max_concurrent_async {
            builder = builder.max_concurrent_async(max);
        }
        if let Some(secs) = config.shutdown_timeout_secs {
            builder = builder.shutdown_timeout(Duration::from_secs(secs));
        }

        let bus = builder.build();
        app.add_component(bus.clone());

        // Auto-register all #[event_listener] functions discovered by inventory
        let listeners: Vec<_> = auto_listeners().collect();
        info!(count = listeners.len(), "summer-jaeb: auto-registering event listeners");
        for registrar in listeners {
            registrar.register(&bus, app).await;
        }
    }
}
