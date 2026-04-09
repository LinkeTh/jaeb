// SPDX-License-Identifier: MIT
use schemars::JsonSchema;
use serde::Deserialize;
use summer::config::Configurable;

/// Configuration for the summer-jaeb plugin.
///
/// All fields are optional; unset fields fall back to the [`jaeb::EventBusBuilder`] defaults.
///
/// Example `app.toml`:
/// ```toml
/// [jaeb]
/// buffer_size = 512
/// handler_timeout_secs = 5
/// max_concurrent_async = 100
/// shutdown_timeout_secs = 10
/// ```
#[derive(Debug, Default, Configurable, JsonSchema, Deserialize)]
#[config_prefix = "jaeb"]
pub struct Config {
    /// Internal channel buffer size. Defaults to 256.
    pub buffer_size: Option<usize>,

    /// Maximum time in seconds a single handler invocation may run before
    /// being treated as a failure. Defaults to no timeout.
    pub handler_timeout_secs: Option<u64>,

    /// Maximum number of async handler tasks that may execute concurrently.
    /// Defaults to unlimited.
    pub max_concurrent_async: Option<usize>,

    /// Maximum time in seconds [`jaeb::EventBus::shutdown`] will wait for in-flight
    /// async tasks to complete. Defaults to waiting indefinitely.
    pub shutdown_timeout_secs: Option<u64>,
}
