use std::time::Instant;

/// Lifecycle events sent from mock handlers and the publisher to the metrics collector.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields are structural; used for Debug output and future extension
pub enum SimEvent {
    Published {
        event_type: u8,
        seq: u64,
        at: Instant,
    },
    HandlerStarted {
        listener: String,
        event_type: u8,
        seq: u64,
        at: Instant,
    },
    HandlerCompleted {
        listener: String,
        event_type: u8,
        seq: u64,
        duration_ms: u64,
        at: Instant,
    },
    HandlerFailed {
        listener: String,
        event_type: u8,
        seq: u64,
        at: Instant,
    },
    DeadLetterReceived {
        listener: String,
        event_type: u8,
        seq: u64,
        error: String,
        at: Instant,
    },
    BackpressureHit {
        at: Instant,
    },
    SimulationDone,
}
