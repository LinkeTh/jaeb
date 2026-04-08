mod config;

use std::time::Duration;

use jaeb::EventBus;
use summer::app::AppBuilder;
use summer::async_trait;
use summer::config::ConfigRegistry;
use summer::plugin::{MutableComponentRegistry, Plugin};

use crate::config::Config;

pub use crate::config::Config as JaebConfig;

/// summer-rs plugin that registers a [`jaeb::EventBus`] as an application component.
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

        app.add_component(builder.build());
    }
}
