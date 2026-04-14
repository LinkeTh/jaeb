use std::fmt;
use std::time::Duration;

/// Top-level simulation configuration built from TUI forms.
#[derive(Clone, Debug)]
pub struct SimConfig {
    pub bus: BusConfigForm,
    pub event_types: Vec<EventTypeConfig>,
    pub listeners: Vec<ListenerConfig>,
    pub publish: PublishConfig,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            bus: BusConfigForm::default(),
            event_types: vec![
                EventTypeConfig {
                    name: "OrderEvent".into(),
                    events_per_sec: 25.0,
                    pattern: PublishPattern::Constant,
                    burst_every_n: 10,
                },
                EventTypeConfig {
                    name: "PaymentEvent".into(),
                    events_per_sec: 25.0,
                    pattern: PublishPattern::Constant,
                    burst_every_n: 10,
                },
            ],
            listeners: vec![
                ListenerConfig {
                    name: "order_handler".into(),
                    event_type_idx: 0,
                    mode: HandlerMode::Async,
                    processing_ms: 50,
                    failure_rate: 0.05,
                    max_retries: 2,
                    retry_strategy: RetryStrategyChoice::Exponential { base_ms: 100, max_ms: 1000 },
                    dead_letter: true,
                },
                ListenerConfig {
                    name: "payment_sync".into(),
                    event_type_idx: 1,
                    mode: HandlerMode::Sync,
                    processing_ms: 20,
                    failure_rate: 0.1,
                    max_retries: 0,
                    retry_strategy: RetryStrategyChoice::None,
                    dead_letter: true,
                },
            ],
            publish: PublishConfig::default(),
        }
    }
}

impl SimConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.event_types.is_empty() {
            return Err("At least 1 event type required".into());
        }
        if self.event_types.len() > 8 {
            return Err("Maximum 8 event types".into());
        }
        let mut normalized = std::collections::HashSet::new();
        for event_type in &self.event_types {
            let trimmed = event_type.name.trim();
            if trimmed.is_empty() {
                return Err("Event type names cannot be empty".into());
            }
            if !normalized.insert(trimmed.to_ascii_lowercase()) {
                return Err(format!("Duplicate event type name: '{trimmed}'"));
            }
            if event_type.events_per_sec <= 0.0 {
                return Err(format!("Event type '{trimmed}' rate must be > 0"));
            }
            if matches!(event_type.pattern, PublishPattern::Burst) && event_type.burst_every_n == 0 {
                return Err(format!("Event type '{trimmed}' burst interval must be > 0"));
            }
        }
        if self.listeners.is_empty() {
            return Err("At least 1 listener required".into());
        }
        if self.listeners.len() > 6 {
            return Err("Maximum 6 listeners".into());
        }
        for (i, l) in self.listeners.iter().enumerate() {
            if l.name.is_empty() {
                return Err(format!("Listener {} has no name", i + 1));
            }
            if l.event_type_idx as usize >= self.event_types.len() {
                return Err(format!("Listener '{}' has invalid event type", l.name));
            }
            if l.failure_rate < 0.0 || l.failure_rate > 1.0 {
                return Err(format!("Listener '{}' failure rate must be 0.0-1.0", l.name));
            }
        }
        match &self.publish.stop_condition {
            StopCondition::Duration(d) if d.is_zero() => return Err("Duration must be > 0".into()),
            StopCondition::TotalEvents(n) if *n == 0 => return Err("Total events must be > 0".into()),
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct EventTypeConfig {
    pub name: String,
    pub events_per_sec: f64,
    pub pattern: PublishPattern,
    pub burst_every_n: u64,
}

impl Default for EventTypeConfig {
    fn default() -> Self {
        Self {
            name: "EventType".into(),
            events_per_sec: 20.0,
            pattern: PublishPattern::Constant,
            burst_every_n: 10,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishPattern {
    Constant,
    Burst,
    Random,
}

impl PublishPattern {
    pub fn cycle_next(&self) -> Self {
        match self {
            Self::Constant => Self::Burst,
            Self::Burst => Self::Random,
            Self::Random => Self::Constant,
        }
    }
}

impl fmt::Display for PublishPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Constant => write!(f, "Constant"),
            Self::Burst => write!(f, "Burst"),
            Self::Random => write!(f, "Random"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BusConfigForm {
    pub handler_timeout_ms: u64,
    pub max_concurrent_async: usize,
    pub shutdown_timeout_ms: u64,
}

impl Default for BusConfigForm {
    fn default() -> Self {
        Self {
            handler_timeout_ms: 0,
            max_concurrent_async: 0,
            shutdown_timeout_ms: 3000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ListenerConfig {
    pub name: String,
    pub event_type_idx: u8,
    pub mode: HandlerMode,
    pub processing_ms: u64,
    pub failure_rate: f64,
    pub max_retries: u32,
    pub retry_strategy: RetryStrategyChoice,
    pub dead_letter: bool,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            event_type_idx: 0,
            mode: HandlerMode::Async,
            processing_ms: 50,
            failure_rate: 0.0,
            max_retries: 0,
            retry_strategy: RetryStrategyChoice::None,
            dead_letter: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandlerMode {
    Sync,
    Async,
}

impl HandlerMode {
    pub fn cycle_next(&self) -> Self {
        match self {
            Self::Sync => Self::Async,
            Self::Async => Self::Sync,
        }
    }
}

impl fmt::Display for HandlerMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sync => write!(f, "Sync"),
            Self::Async => write!(f, "Async"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetryStrategyChoice {
    None,
    Fixed { delay_ms: u64 },
    Exponential { base_ms: u64, max_ms: u64 },
    ExponentialWithJitter { base_ms: u64, max_ms: u64 },
}

impl RetryStrategyChoice {
    #[allow(dead_code)] // available for config UI extensions
    pub const VARIANTS: &[&str] = &["None", "Fixed", "Exponential", "Exp+Jitter"];

    #[allow(dead_code)] // available for config UI extensions
    pub fn variant_index(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Fixed { .. } => 1,
            Self::Exponential { .. } => 2,
            Self::ExponentialWithJitter { .. } => 3,
        }
    }

    pub fn cycle_next(&self) -> Self {
        match self {
            Self::None => Self::Fixed { delay_ms: 100 },
            Self::Fixed { .. } => Self::Exponential { base_ms: 100, max_ms: 2000 },
            Self::Exponential { .. } => Self::ExponentialWithJitter { base_ms: 100, max_ms: 2000 },
            Self::ExponentialWithJitter { .. } => Self::None,
        }
    }
}

impl fmt::Display for RetryStrategyChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Fixed { delay_ms } => write!(f, "Fixed({}ms)", delay_ms),
            Self::Exponential { base_ms, max_ms } => write!(f, "Exp({}ms,{}ms)", base_ms, max_ms),
            Self::ExponentialWithJitter { base_ms, max_ms } => write!(f, "Jitter({}ms,{}ms)", base_ms, max_ms),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PublishConfig {
    pub stop_condition: StopCondition,
}

impl Default for PublishConfig {
    fn default() -> Self {
        Self {
            stop_condition: StopCondition::Duration(Duration::from_secs(30)),
        }
    }
}

#[derive(Clone, Debug)]
pub enum StopCondition {
    Duration(Duration),
    TotalEvents(usize),
}

impl StopCondition {
    pub fn cycle_next(&self) -> Self {
        match self {
            Self::Duration(_) => Self::TotalEvents(1000),
            Self::TotalEvents(_) => Self::Duration(Duration::from_secs(30)),
        }
    }

    pub fn is_duration(&self) -> bool {
        matches!(self, Self::Duration(_))
    }
}

impl fmt::Display for StopCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duration(d) => write!(f, "{}s", d.as_secs()),
            Self::TotalEvents(n) => write!(f, "{} processed", n),
        }
    }
}
