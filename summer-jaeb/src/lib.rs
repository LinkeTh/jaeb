mod config;

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use jaeb::EventBus;
use summer::app::AppBuilder;
use summer::async_trait;
use summer::config::ConfigRegistry;
use summer::plugin::{MutableComponentRegistry, Plugin};
use tracing::{error, info};

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
/// # Plugin Dependencies
///
/// If your `#[event_listener]` functions inject components (via `Component<T>`),
/// those components must be registered by plugins that are built **before**
/// `SummerJaeb`. Declare the providing plugins as dependencies so the summer-rs
/// runtime can enforce the correct build order:
///
/// ```rust,ignore
/// use summer::App;
/// use summer_jaeb::SummerJaeb;
///
/// App::new()
///     .add_plugin(DbPoolPlugin)
///     .add_plugin(SummerJaeb::new().with_dependency("DbPoolPlugin"))
///     .run()
///     .await;
/// ```
///
/// The runtime guarantees that `DbPoolPlugin` is built before `SummerJaeb`,
/// regardless of the order they are added.
///
/// # Shutdown
///
/// The summer `Plugin` trait does not provide a teardown hook, so this plugin
/// **does not** call [`EventBus::shutdown()`] automatically.  If you need a
/// graceful drain of in-flight handlers, retrieve the bus from the component
/// registry and call `shutdown()` manually before the application exits:
///
/// ```rust,ignore
/// let bus: EventBus = app.get_component().unwrap();
/// bus.shutdown().await.expect("shutdown");
/// ```
///
/// # Panics
///
/// The plugin panics during [`build`](Plugin::build) if:
/// - The resulting `EventBusBuilder` configuration is invalid (e.g. zero buffer size).
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
/// use summer::App;
/// use summer_jaeb::SummerJaeb;
///
/// #[tokio::main]
/// async fn main() {
///     App::new()
///         .add_plugin(SummerJaeb::new())
///         .run()
///         .await;
/// }
/// ```
///
/// Configuration (`app.toml`):
/// ```toml
/// [jaeb]
/// handler_timeout_secs = 5
/// max_concurrent_async = 100
/// shutdown_timeout_secs = 10
/// ```
///
/// If the `[jaeb]` section is missing or cannot be parsed, the plugin falls
/// back to builder defaults and logs a warning.
///
/// Retrieve the bus elsewhere via dependency injection:
/// ```rust,ignore
/// use jaeb::EventBus;
/// use summer::plugin::ComponentRegistry;
///
/// let bus: EventBus = app.get_component().unwrap();
/// ```
pub struct SummerJaeb {
    deps: Vec<String>,
}

impl SummerJaeb {
    /// Create a new `SummerJaeb` plugin with no declared dependencies.
    pub fn new() -> Self {
        Self { deps: Vec::new() }
    }

    /// Declare that this plugin depends on another named plugin.
    ///
    /// The summer-rs runtime will build the named plugin before building
    /// `SummerJaeb`. Can be chained: `.with_dependency("A").with_dependency("B")`.
    pub fn with_dependency(mut self, name: impl Into<String>) -> Self {
        self.deps.push(name.into());
        self
    }

    /// Declare multiple plugin dependencies at once.
    ///
    /// ```rust,ignore
    /// SummerJaeb::new().with_dependencies(["DbPoolPlugin", "CachePlugin"])
    /// ```
    pub fn with_dependencies(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.deps.extend(names.into_iter().map(Into::into));
        self
    }
}

impl Default for SummerJaeb {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for SummerJaeb {
    async fn build(&self, app: &mut AppBuilder) {
        let config = app.get_config::<Config>().unwrap_or_else(|err| {
            tracing::warn!(error = %err, "summer-jaeb: failed to load [jaeb] config, using defaults");
            Config::default()
        });

        let mut builder = EventBus::builder();

        if let Some(secs) = config.handler_timeout_secs {
            builder = builder.handler_timeout(Duration::from_secs(secs));
        }
        if let Some(max) = config.max_concurrent_async {
            builder = builder.max_concurrent_async(max);
        }
        if let Some(secs) = config.shutdown_timeout_secs {
            builder = builder.shutdown_timeout(Duration::from_secs(secs));
        }

        let bus = match builder.build().await {
            Ok(bus) => bus,
            Err(err) => {
                // A bad configuration is a fatal
                // startup error — there is no sensible fallback.
                error!(error = %err, "summer-jaeb: invalid event bus configuration");
                panic!("summer-jaeb: {err}");
            }
        };
        app.add_component(bus.clone());

        // Auto-register all #[event_listener] functions discovered by inventory
        let listeners: Vec<_> = auto_listeners().collect();
        info!(count = listeners.len(), "summer-jaeb: auto-registering event listeners");
        for registrar in listeners {
            registrar.register(&bus, app).await;
        }
    }

    fn name(&self) -> &str {
        "SummerJaeb"
    }

    fn dependencies(&self) -> Vec<&str> {
        self.deps.iter().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use summer::plugin::Plugin;

    #[test]
    fn default_has_no_dependencies() {
        let plugin = SummerJaeb::default();
        assert!(plugin.dependencies().is_empty());
    }

    #[test]
    fn new_has_no_dependencies() {
        let plugin = SummerJaeb::new();
        assert!(plugin.dependencies().is_empty());
    }

    #[test]
    fn with_dependency_adds_single_dep() {
        let plugin = SummerJaeb::new().with_dependency("DbPoolPlugin");
        assert_eq!(plugin.dependencies(), vec!["DbPoolPlugin"]);
    }

    #[test]
    fn with_dependencies_adds_multiple_deps() {
        let plugin = SummerJaeb::new().with_dependencies(["A", "B", "C"]);
        assert_eq!(plugin.dependencies(), vec!["A", "B", "C"]);
    }

    #[test]
    fn chained_with_dependency() {
        let plugin = SummerJaeb::new().with_dependency("First").with_dependency("Second");
        assert_eq!(plugin.dependencies(), vec!["First", "Second"]);
    }
}
