use std::collections::VecDeque;
use std::time::Instant;

/// All visualization data, updated by the collector, read by the TUI renderer.
pub struct VisualizationState {
    // Counters
    pub total_published: u64,
    pub total_handled: u64,
    pub total_failed: u64,
    pub total_dead_letters: u64,
    pub in_flight: i64,
    pub backpressure_hits: u64,

    // Sparkline data (ring buffers, sampled every 500ms)
    pub throughput_history: VecDeque<u64>,
    pub backpressure_history: VecDeque<u64>,
    // Snapshot counters for computing deltas
    pub last_sample_published: u64,
    pub last_sample_bp: u64,

    // Channel pressure
    pub buffer_size: usize,

    // Flow animation blips
    pub flow_blips: VecDeque<FlowBlip>,

    // Dead letter log (capped at 200)
    pub dead_letters: VecDeque<DeadLetterEntry>,

    // From bus.stats()
    pub bus_in_flight_async: usize,
    pub bus_total_subscriptions: usize,

    // Lifecycle
    pub sim_start: Option<Instant>,
    pub sim_done: bool,
}

const MAX_SPARKLINE_SAMPLES: usize = 60;
const MAX_DEAD_LETTERS: usize = 200;
const MAX_FLOW_BLIPS: usize = 30;

impl VisualizationState {
    pub fn new(buffer_size: usize) -> Self {
        Self {
            total_published: 0,
            total_handled: 0,
            total_failed: 0,
            total_dead_letters: 0,
            in_flight: 0,
            backpressure_hits: 0,
            throughput_history: VecDeque::with_capacity(MAX_SPARKLINE_SAMPLES),
            backpressure_history: VecDeque::with_capacity(MAX_SPARKLINE_SAMPLES),
            last_sample_published: 0,
            last_sample_bp: 0,
            buffer_size,
            flow_blips: VecDeque::with_capacity(MAX_FLOW_BLIPS),
            dead_letters: VecDeque::with_capacity(MAX_DEAD_LETTERS),
            bus_in_flight_async: 0,
            bus_total_subscriptions: 0,
            sim_start: None,
            sim_done: false,
        }
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.sim_start.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
    }

    pub fn current_rate(&self) -> f64 {
        let elapsed = self.elapsed_secs();
        if elapsed > 0.0 { self.total_published as f64 / elapsed } else { 0.0 }
    }

    pub fn pressure_ratio(&self) -> f64 {
        if self.buffer_size == 0 {
            return 0.0;
        }
        // Use in-flight as a proxy for channel utilization
        (self.in_flight.max(0) as f64 / self.buffer_size as f64).min(1.0)
    }

    /// Called periodically (every 500ms) to sample throughput deltas.
    pub fn sample_throughput(&mut self) {
        let pub_delta = self.total_published - self.last_sample_published;
        let bp_delta = self.backpressure_hits - self.last_sample_bp;
        self.last_sample_published = self.total_published;
        self.last_sample_bp = self.backpressure_hits;

        self.throughput_history.push_back(pub_delta);
        if self.throughput_history.len() > MAX_SPARKLINE_SAMPLES {
            self.throughput_history.pop_front();
        }

        self.backpressure_history.push_back(bp_delta);
        if self.backpressure_history.len() > MAX_SPARKLINE_SAMPLES {
            self.backpressure_history.pop_front();
        }
    }

    pub fn add_flow_blip(&mut self, blip: FlowBlip) {
        self.flow_blips.push_back(blip);
        if self.flow_blips.len() > MAX_FLOW_BLIPS {
            self.flow_blips.pop_front();
        }
    }

    pub fn add_dead_letter(&mut self, entry: DeadLetterEntry) {
        self.dead_letters.push_back(entry);
        if self.dead_letters.len() > MAX_DEAD_LETTERS {
            self.dead_letters.pop_front();
        }
    }

    /// Advance flow blips and remove completed ones.
    pub fn tick_flow_blips(&mut self, dt_secs: f64) {
        for blip in self.flow_blips.iter_mut() {
            blip.progress += dt_secs / blip.ttl_secs;
        }
        self.flow_blips.retain(|b| b.progress < 1.0);
    }
}

#[derive(Clone, Debug)]
pub struct FlowBlip {
    pub event_type: u8,
    pub progress: f64,
    #[allow(dead_code)] // structural: used for Debug output
    pub spawned_at: Instant,
    pub ttl_secs: f64,
}

#[derive(Clone, Debug)]
pub struct DeadLetterEntry {
    #[allow(dead_code)] // structural: used for Debug output
    pub at: Instant,
    pub elapsed_secs: f64,
    pub listener: String,
    pub event_type: String,
    pub seq: u64,
    pub reason: String,
}
