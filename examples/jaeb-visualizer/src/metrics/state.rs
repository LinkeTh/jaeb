use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use crate::config::types::ListenerConfig;

/// All visualization data, updated by the collector, read by the TUI renderer.
pub struct VisualizationState {
    // Counters
    pub total_published: u64,
    pub total_handled: u64,
    pub total_failed: u64,
    pub total_dead_letters: u64,
    pub in_flight: i64,
    pub backpressure_hits: u64,

    // Global sampled data (ring buffers, sampled every 500ms)
    pub throughput_history: VecDeque<u64>,
    pub backpressure_history: VecDeque<u64>,
    /// Sampled handler-level in-flight count (HandlerStarted minus completions/failures).
    pub in_flight_history: VecDeque<i64>,
    /// Sampled bus_in_flight_async count (spawned async handler tasks still alive).
    pub async_tasks_history: VecDeque<usize>,
    // Snapshot counters for computing deltas
    pub last_sample_published: u64,
    pub last_sample_bp: u64,

    // Handler panels
    pub handler_metrics: Vec<HandlerViz>,

    // Dead letter log (capped)
    pub dead_letters: VecDeque<DeadLetterEntry>,

    // From bus.stats()
    pub bus_in_flight_async: usize,
    pub bus_dispatches_in_flight: usize,
    pub bus_total_subscriptions: usize,

    // Lifecycle
    pub sim_start: Option<Instant>,
    pub sim_end: Option<Instant>,
    pub sim_done: bool,
    pub publishing_stopped: bool,
    pub frozen_elapsed_secs: Option<f64>,
    pub frozen_avg_rate: Option<f64>,
    pub total_processed: Arc<AtomicU64>,
}

const MAX_SPARKLINE_SAMPLES: usize = 60;
const MAX_DEAD_LETTERS: usize = 200;

impl VisualizationState {
    pub fn new(listeners: &[ListenerConfig]) -> Self {
        Self {
            total_published: 0,
            total_handled: 0,
            total_failed: 0,
            total_dead_letters: 0,
            in_flight: 0,
            backpressure_hits: 0,
            throughput_history: VecDeque::with_capacity(MAX_SPARKLINE_SAMPLES),
            backpressure_history: VecDeque::with_capacity(MAX_SPARKLINE_SAMPLES),
            in_flight_history: VecDeque::with_capacity(MAX_SPARKLINE_SAMPLES),
            async_tasks_history: VecDeque::with_capacity(MAX_SPARKLINE_SAMPLES),
            last_sample_published: 0,
            last_sample_bp: 0,
            handler_metrics: listeners
                .iter()
                .enumerate()
                .map(|(idx, cfg)| HandlerViz::new(idx, cfg.name.clone(), cfg.event_type_idx as usize))
                .collect(),
            dead_letters: VecDeque::with_capacity(MAX_DEAD_LETTERS),
            bus_in_flight_async: 0,
            bus_dispatches_in_flight: 0,
            bus_total_subscriptions: 0,
            sim_start: None,
            sim_end: None,
            sim_done: false,
            publishing_stopped: false,
            frozen_elapsed_secs: None,
            frozen_avg_rate: None,
            total_processed: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn elapsed_secs(&self) -> f64 {
        if let Some(frozen) = self.frozen_elapsed_secs {
            return frozen;
        }
        self.sim_start.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
    }

    pub fn current_rate(&self) -> f64 {
        if let Some(frozen) = self.frozen_avg_rate {
            return frozen;
        }
        let elapsed = self.elapsed_secs();
        if elapsed > 0.0 { self.total_published as f64 / elapsed } else { 0.0 }
    }

    /// Overall average handler latency across all handlers (latest sample window).
    pub fn avg_latency_ms(&self) -> Option<f64> {
        let active: Vec<f64> = self
            .handler_metrics
            .iter()
            .filter_map(|h| h.latency_history.back().copied().filter(|&v| v > 0.0))
            .collect();
        if active.is_empty() {
            return None;
        }
        Some(active.iter().sum::<f64>() / active.len() as f64)
    }

    /// Overall min latency across all handlers (all-time).
    pub fn global_min_latency_ms(&self) -> Option<f64> {
        self.handler_metrics.iter().filter_map(|h| h.min_latency_ms).reduce(f64::min)
    }

    /// Overall max latency across all handlers (all-time).
    pub fn global_max_latency_ms(&self) -> f64 {
        self.handler_metrics.iter().map(|h| h.max_latency_ms).fold(0.0_f64, f64::max)
    }

    pub fn freeze_on_done(&mut self) {
        if self.frozen_elapsed_secs.is_some() {
            return;
        }
        let elapsed = self.sim_start.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
        self.frozen_elapsed_secs = Some(elapsed);
        self.frozen_avg_rate = Some(if elapsed > 0.0 { self.total_published as f64 / elapsed } else { 0.0 });
        for handler in &mut self.handler_metrics {
            handler.frozen_rate = Some(handler.current_rate());
            handler.frozen_avg_latency_ms = handler.latency_history.back().copied();
        }
    }

    /// Called periodically (every 500ms) to sample throughput and latency deltas.
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

        self.in_flight_history.push_back(self.in_flight);
        if self.in_flight_history.len() > MAX_SPARKLINE_SAMPLES {
            self.in_flight_history.pop_front();
        }

        self.async_tasks_history.push_back(self.bus_in_flight_async);
        if self.async_tasks_history.len() > MAX_SPARKLINE_SAMPLES {
            self.async_tasks_history.pop_front();
        }

        for handler in &mut self.handler_metrics {
            handler.sample();
        }
    }

    pub fn add_dead_letter(&mut self, entry: DeadLetterEntry) {
        self.dead_letters.push_back(entry);
        if self.dead_letters.len() > MAX_DEAD_LETTERS {
            self.dead_letters.pop_front();
        }
    }

    pub fn on_handler_started(&mut self, listener_idx: usize, event_type_idx: usize, seq: u64, at: Instant) {
        let _ = (event_type_idx, seq, at);
        if let Some(handler) = self.handler_metrics.get_mut(listener_idx) {
            handler.in_flight += 1;
        }
    }

    pub fn on_handler_completed(&mut self, listener_idx: usize, duration_ms: u64) {
        if let Some(handler) = self.handler_metrics.get_mut(listener_idx) {
            handler.total_handled += 1;
            handler.completed_since_sample += 1;
            handler.in_flight = handler.in_flight.saturating_sub(1);
            let dur = duration_ms as f64;
            handler.latency_sum_since_sample += dur;
            handler.latency_count_since_sample += 1;
            handler.min_latency_ms = Some(match handler.min_latency_ms {
                Some(prev) => prev.min(dur),
                None => dur,
            });
            if dur > handler.max_latency_ms {
                handler.max_latency_ms = dur;
            }
        }
    }

    pub fn on_handler_failed(&mut self, listener_idx: usize) {
        if let Some(handler) = self.handler_metrics.get_mut(listener_idx) {
            handler.total_failed += 1;
            handler.failed_since_sample += 1;
            handler.in_flight = handler.in_flight.saturating_sub(1);
        }
    }
}

pub struct HandlerViz {
    #[allow(dead_code)]
    pub listener_idx: usize,
    pub name: String,
    #[allow(dead_code)]
    pub event_type_idx: usize,
    pub total_handled: u64,
    pub total_failed: u64,
    pub in_flight: i64,

    // Throughput
    pub throughput_history: VecDeque<u64>,
    pub completed_since_sample: u64,
    pub failed_since_sample: u64,
    pub frozen_rate: Option<f64>,

    // Latency (avg per 500ms window, milliseconds)
    pub latency_history: VecDeque<f64>,
    pub latency_sum_since_sample: f64,
    pub latency_count_since_sample: u64,
    pub min_latency_ms: Option<f64>,
    pub max_latency_ms: f64,
    pub frozen_avg_latency_ms: Option<f64>,
}

impl HandlerViz {
    pub fn new(listener_idx: usize, name: String, event_type_idx: usize) -> Self {
        Self {
            listener_idx,
            name,
            event_type_idx,
            total_handled: 0,
            total_failed: 0,
            in_flight: 0,
            throughput_history: VecDeque::with_capacity(MAX_SPARKLINE_SAMPLES),
            completed_since_sample: 0,
            failed_since_sample: 0,
            frozen_rate: None,
            latency_history: VecDeque::with_capacity(MAX_SPARKLINE_SAMPLES),
            latency_sum_since_sample: 0.0,
            latency_count_since_sample: 0,
            min_latency_ms: None,
            max_latency_ms: 0.0,
            frozen_avg_latency_ms: None,
        }
    }

    pub fn sample(&mut self) {
        // Throughput sample
        self.throughput_history.push_back(self.completed_since_sample);
        self.completed_since_sample = 0;
        self.failed_since_sample = 0;
        if self.throughput_history.len() > MAX_SPARKLINE_SAMPLES {
            self.throughput_history.pop_front();
        }

        // Latency sample: compute avg for this window
        let avg_latency = if self.latency_count_since_sample > 0 {
            self.latency_sum_since_sample / self.latency_count_since_sample as f64
        } else {
            // Carry forward the previous value so the chart doesn't drop to 0 during idle windows
            self.latency_history.back().copied().unwrap_or(0.0)
        };
        self.latency_sum_since_sample = 0.0;
        self.latency_count_since_sample = 0;

        self.latency_history.push_back(avg_latency);
        if self.latency_history.len() > MAX_SPARKLINE_SAMPLES {
            self.latency_history.pop_front();
        }
    }

    pub fn current_rate(&self) -> f64 {
        self.throughput_history.back().map(|v| *v as f64 / 0.5).unwrap_or(0.0)
    }

    /// Latest avg latency from the rolling window, or the frozen value after done.
    pub fn current_avg_latency_ms(&self) -> f64 {
        self.frozen_avg_latency_ms.or_else(|| self.latency_history.back().copied()).unwrap_or(0.0)
    }
}

#[derive(Clone, Debug)]
pub struct DeadLetterEntry {
    #[allow(dead_code)]
    pub at: Instant,
    pub elapsed_secs: f64,
    pub listener: String,
    #[allow(dead_code)]
    pub listener_idx: Option<usize>,
    pub event_type: String,
    pub seq: u64,
    pub reason: String,
}
