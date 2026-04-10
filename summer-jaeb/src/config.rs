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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_all_none() {
        let cfg = Config::default();
        assert!(cfg.buffer_size.is_none());
        assert!(cfg.handler_timeout_secs.is_none());
        assert!(cfg.max_concurrent_async.is_none());
        assert!(cfg.shutdown_timeout_secs.is_none());
    }

    #[test]
    fn config_full_deserialize() {
        let json = r#"{
            "buffer_size": 512,
            "handler_timeout_secs": 5,
            "max_concurrent_async": 100,
            "shutdown_timeout_secs": 10
        }"#;
        let cfg: Config = serde_json::from_str(json).expect("deserialize full config");
        assert_eq!(cfg.buffer_size, Some(512));
        assert_eq!(cfg.handler_timeout_secs, Some(5));
        assert_eq!(cfg.max_concurrent_async, Some(100));
        assert_eq!(cfg.shutdown_timeout_secs, Some(10));
    }

    #[test]
    fn config_partial_deserialize() {
        let json = r#"{ "buffer_size": 1024 }"#;
        let cfg: Config = serde_json::from_str(json).expect("deserialize partial config");
        assert_eq!(cfg.buffer_size, Some(1024));
        assert!(cfg.handler_timeout_secs.is_none());
        assert!(cfg.max_concurrent_async.is_none());
        assert!(cfg.shutdown_timeout_secs.is_none());
    }

    #[test]
    fn config_empty_object_deserialize() {
        let json = "{}";
        let cfg: Config = serde_json::from_str(json).expect("deserialize empty config");
        assert!(cfg.buffer_size.is_none());
        assert!(cfg.handler_timeout_secs.is_none());
        assert!(cfg.max_concurrent_async.is_none());
        assert!(cfg.shutdown_timeout_secs.is_none());
    }
}
