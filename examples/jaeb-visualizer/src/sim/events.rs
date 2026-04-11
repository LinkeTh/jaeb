use std::time::Instant;

/// Lifecycle events sent from mock handlers and the publisher to the metrics collector.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields are structural; used for Debug output and future extension
pub enum SimEvent {
    Published {
        event_type_idx: usize,
        seq: u64,
        at: Instant,
    },
    HandlerStarted {
        listener: String,
        listener_idx: usize,
        event_type_idx: usize,
        seq: u64,
        at: Instant,
    },
    HandlerCompleted {
        listener: String,
        listener_idx: usize,
        event_type_idx: usize,
        seq: u64,
        duration_ms: u64,
        at: Instant,
    },
    HandlerFailed {
        listener: String,
        listener_idx: usize,
        event_type_idx: usize,
        seq: u64,
        at: Instant,
    },
    DeadLetterReceived {
        listener: String,
        listener_idx: Option<usize>,
        event_type_idx: usize,
        seq: u64,
        error: String,
        at: Instant,
    },
    BackpressureHit {
        at: Instant,
    },
    PublishingStopped {
        total_published: u64,
    },
    SimulationDone,
}
