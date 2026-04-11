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
    // Snapshot counters for computing deltas
    pub last_sample_published: u64,
    pub last_sample_bp: u64,

    // Channel pressure
    pub buffer_size: usize,

    // Global flow animation blips
    pub flow_blips: VecDeque<FlowBlip>,

    // Handler panels
    pub handler_metrics: Vec<HandlerViz>,

    // Dead letter log (capped)
    pub dead_letters: VecDeque<DeadLetterEntry>,

    // From bus.stats()
    pub bus_in_flight_async: usize,
    pub bus_publish_in_flight: usize,
    pub bus_queue_capacity: usize,
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
const MAX_FLOW_BLIPS: usize = 30;
const MAX_HANDLER_FLOW_BLIPS: usize = 20;

impl VisualizationState {
    pub fn new(buffer_size: usize, listeners: &[ListenerConfig]) -> Self {
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
            handler_metrics: listeners
                .iter()
                .enumerate()
                .map(|(idx, cfg)| HandlerViz::new(idx, cfg.name.clone(), cfg.event_type_idx as usize))
                .collect(),
            dead_letters: VecDeque::with_capacity(MAX_DEAD_LETTERS),
            bus_in_flight_async: 0,
            bus_publish_in_flight: 0,
            bus_queue_capacity: buffer_size,
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

    pub fn freeze_on_done(&mut self) {
        if self.frozen_elapsed_secs.is_some() {
            return;
        }
        let elapsed = self.sim_start.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
        self.frozen_elapsed_secs = Some(elapsed);
        self.frozen_avg_rate = Some(if elapsed > 0.0 { self.total_published as f64 / elapsed } else { 0.0 });
        for handler in &mut self.handler_metrics {
            handler.frozen_rate = Some(handler.current_rate());
        }
    }

    pub fn pressure_ratio(&self) -> f64 {
        let capacity = if self.bus_queue_capacity == 0 {
            self.buffer_size
        } else {
            self.bus_queue_capacity
        };
        if capacity == 0 {
            return 0.0;
        }
        (self.bus_publish_in_flight as f64 / capacity as f64).min(1.0)
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

        for handler in &mut self.handler_metrics {
            handler.sample();
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

    pub fn on_handler_started(&mut self, listener_idx: usize, event_type_idx: usize, seq: u64, at: Instant) {
        if let Some(handler) = self.handler_metrics.get_mut(listener_idx) {
            handler.in_flight += 1;
            handler.add_flow_blip(HandlerFlowBlip {
                event_type_idx,
                seq,
                progress: 0.0,
                spawned_at: at,
                ttl_secs: 1.6,
                success: None,
            });
        }
    }

    pub fn on_handler_completed(&mut self, listener_idx: usize, seq: u64) {
        if let Some(handler) = self.handler_metrics.get_mut(listener_idx) {
            handler.total_handled += 1;
            handler.completed_since_sample += 1;
            handler.in_flight = handler.in_flight.saturating_sub(1);
            if let Some(blip) = handler.flow_blips.iter_mut().rev().find(|blip| blip.seq == seq && blip.success.is_none()) {
                blip.success = Some(true);
            }
        }
    }

    pub fn on_handler_failed(&mut self, listener_idx: usize, seq: u64) {
        if let Some(handler) = self.handler_metrics.get_mut(listener_idx) {
            handler.total_failed += 1;
            handler.failed_since_sample += 1;
            handler.in_flight = handler.in_flight.saturating_sub(1);
            if let Some(blip) = handler.flow_blips.iter_mut().rev().find(|blip| blip.seq == seq && blip.success.is_none()) {
                blip.success = Some(false);
            }
        }
    }

    /// Advance flow blips and remove completed ones.
    pub fn tick_flow_blips(&mut self, dt_secs: f64) {
        for blip in self.flow_blips.iter_mut() {
            blip.progress += dt_secs / blip.ttl_secs;
        }
        self.flow_blips.retain(|b| b.progress < 1.0);

        for handler in &mut self.handler_metrics {
            for blip in handler.flow_blips.iter_mut() {
                blip.progress += dt_secs / blip.ttl_secs;
            }
            handler.flow_blips.retain(|b| b.progress < 1.0);
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
    pub throughput_history: VecDeque<u64>,
    pub completed_since_sample: u64,
    pub failed_since_sample: u64,
    pub flow_blips: VecDeque<HandlerFlowBlip>,
    pub frozen_rate: Option<f64>,
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
            flow_blips: VecDeque::with_capacity(MAX_HANDLER_FLOW_BLIPS),
            frozen_rate: None,
        }
    }

    pub fn sample(&mut self) {
        self.throughput_history.push_back(self.completed_since_sample);
        self.completed_since_sample = 0;
        self.failed_since_sample = 0;
        if self.throughput_history.len() > MAX_SPARKLINE_SAMPLES {
            self.throughput_history.pop_front();
        }
    }

    pub fn current_rate(&self) -> f64 {
        self.throughput_history.back().map(|v| *v as f64 / 0.5).unwrap_or(0.0)
    }

    pub fn add_flow_blip(&mut self, blip: HandlerFlowBlip) {
        self.flow_blips.push_back(blip);
        if self.flow_blips.len() > MAX_HANDLER_FLOW_BLIPS {
            self.flow_blips.pop_front();
        }
    }
}

#[derive(Clone, Debug)]
pub struct FlowBlip {
    pub event_type_idx: usize,
    pub progress: f64,
    #[allow(dead_code)]
    pub spawned_at: Instant,
    pub ttl_secs: f64,
}

#[derive(Clone, Debug)]
pub struct HandlerFlowBlip {
    #[allow(dead_code)]
    pub event_type_idx: usize,
    pub seq: u64,
    pub progress: f64,
    #[allow(dead_code)]
    pub spawned_at: Instant,
    pub ttl_secs: f64,
    pub success: Option<bool>,
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
