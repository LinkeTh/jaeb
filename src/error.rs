// SPDX-License-Identifier: MIT
use std::fmt;

pub type HandlerError = Box<dyn std::error::Error + Send + Sync + 'static>;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventBusError {
    ActorStopped,
    ChannelFull,
    ShutdownTimeout,
    /// The builder configuration is invalid.
    InvalidConfig(ConfigError),
    /// A middleware rejected the event before it reached any listener.
    MiddlewareRejected(String),
}

impl fmt::Display for EventBusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ActorStopped => write!(f, "event bus actor has stopped"),
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
            Self::ActorStopped | Self::ChannelFull | Self::ShutdownTimeout | Self::MiddlewareRejected(_) => None,
        }
    }
}

impl From<ConfigError> for EventBusError {
    fn from(err: ConfigError) -> Self {
        Self::InvalidConfig(err)
    }
}
