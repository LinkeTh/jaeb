use std::fmt;

pub type HandlerError = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type HandlerResult = Result<(), HandlerError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventBusError {
    ActorStopped,
    ChannelFull,
    ShutdownTimeout,
}

impl fmt::Display for EventBusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ActorStopped => write!(f, "event bus actor has stopped"),
            Self::ChannelFull => write!(f, "event bus channel is full"),
            Self::ShutdownTimeout => write!(f, "shutdown timed out waiting for in-flight tasks"),
        }
    }
}

impl std::error::Error for EventBusError {}
