use std::fmt;

/// The error type returned by [`EventHandler::handle`](crate::EventHandler::handle)
/// and [`SyncEventHandler::handle`](crate::SyncEventHandler::handle) on failure.
///
/// This is a type-erased, heap-allocated error that can carry any `Send +
/// Sync` error value.
pub type HandlerError = Box<dyn std::error::Error + Send + Sync + 'static>;
/// The result type for handler methods.
///
/// Return `Ok(())` to indicate success, or `Err(e)` to signal a failure that
/// will be processed according to the listener's [`FailurePolicy`](crate::FailurePolicy).
pub type HandlerResult = Result<(), HandlerError>;

/// Specific reason why an [`EventBus`](crate::EventBus) configuration is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// `buffer_size` was set to zero.
    ZeroBufferSize,
    /// `max_concurrent_async` was set to zero.
    ZeroConcurrency,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroBufferSize => write!(f, "buffer_size must be greater than zero"),
            Self::ZeroConcurrency => write!(f, "max_concurrent_async must be greater than zero"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Errors returned by [`EventBus`](crate::EventBus) publish and shutdown
/// operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventBusError {
    /// The bus has been shut down.
    ///
    /// Returned by all publish, subscribe, unsubscribe, and stats calls made
    /// after [`EventBus::shutdown`](crate::EventBus::shutdown) has been
    /// invoked.
    Stopped,
    /// The internal channel buffer is full and no capacity is available.
    ///
    /// Only returned by [`EventBus::try_publish`](crate::EventBus::try_publish).
    /// Use [`EventBus::publish`](crate::EventBus::publish) to wait for
    /// available capacity instead.
    ChannelFull,
    /// Shutdown timed out before all in-flight async tasks completed.
    ///
    /// Remaining tasks were aborted. Returned by
    /// [`EventBus::shutdown`](crate::EventBus::shutdown) when a
    /// [`shutdown_timeout`](crate::EventBusBuilder::shutdown_timeout) was
    /// configured and elapsed.
    ShutdownTimeout,
    /// The builder configuration is invalid.
    InvalidConfig(ConfigError),
    /// A middleware rejected the event before it reached any listener.
    MiddlewareRejected(String),
}

impl fmt::Display for EventBusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => write!(f, "event bus is stopped"),
            Self::ChannelFull => write!(f, "event bus channel is full"),
            Self::ShutdownTimeout => write!(f, "shutdown timed out waiting for in-flight tasks"),
            Self::InvalidConfig(err) => write!(f, "invalid event bus configuration: {err}"),
            Self::MiddlewareRejected(reason) => write!(f, "middleware rejected event: {reason}"),
        }
    }
}

impl std::error::Error for EventBusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidConfig(err) => Some(err),
            Self::Stopped | Self::ChannelFull | Self::ShutdownTimeout | Self::MiddlewareRejected(_) => None,
        }
    }
}

impl From<ConfigError> for EventBusError {
    fn from(err: ConfigError) -> Self {
        Self::InvalidConfig(err)
    }
}
