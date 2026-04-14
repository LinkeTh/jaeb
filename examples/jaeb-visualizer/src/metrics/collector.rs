use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::config::types::ListenerConfig;
use crate::metrics::state::{DeadLetterEntry, VisualizationState};
use crate::sim::SimEvent;

/// Drains SimEvent from the channel and updates VisualizationState.
pub async fn run_collector(
    mut rx: mpsc::UnboundedReceiver<SimEvent>,
    state: Arc<Mutex<VisualizationState>>,
    event_type_names: Vec<String>,
    listeners: Vec<ListenerConfig>,
) {
    while let Some(event) = rx.recv().await {
        let mut s = state.lock().expect("viz state lock poisoned");
        match event {
            SimEvent::Published { event_type_idx, seq, at } => {
                let _ = (event_type_idx, seq, at);
                s.total_published += 1;
            }
            SimEvent::HandlerStarted {
                listener_idx,
                event_type_idx,
                seq,
                at,
                ..
            } => {
                s.in_flight += 1;
                s.on_handler_started(listener_idx, event_type_idx, seq, at);
            }
            SimEvent::HandlerCompleted {
                listener_idx,
                seq,
                duration_ms,
                ..
            } => {
                let _ = seq;
                s.total_handled += 1;
                s.in_flight = s.in_flight.saturating_sub(1);
                s.on_handler_completed(listener_idx, duration_ms);
                s.total_processed.fetch_add(1, Ordering::Relaxed);
            }
            SimEvent::HandlerFailed { listener_idx, seq, .. } => {
                let _ = seq;
                s.total_failed += 1;
                s.in_flight = s.in_flight.saturating_sub(1);
                s.on_handler_failed(listener_idx);
            }
            SimEvent::DeadLetterReceived {
                listener,
                listener_idx,
                event_type_idx,
                seq,
                error,
                at,
            } => {
                s.total_dead_letters += 1;
                s.total_processed.fetch_add(1, Ordering::Relaxed);
                let elapsed = s.sim_start.map(|start| at.duration_since(start).as_secs_f64()).unwrap_or(0.0);
                let type_name = event_type_names
                    .get(event_type_idx)
                    .cloned()
                    .unwrap_or_else(|| format!("Type{event_type_idx}"));
                let mapped_listener_idx = listener_idx.or_else(|| {
                    listeners
                        .iter()
                        .enumerate()
                        .find_map(|(idx, l)| if l.name == listener { Some(idx) } else { None })
                });
                s.add_dead_letter(DeadLetterEntry {
                    at,
                    elapsed_secs: elapsed,
                    listener,
                    listener_idx: mapped_listener_idx,
                    event_type: type_name,
                    seq,
                    reason: error,
                });
            }
            SimEvent::BackpressureHit { .. } => {
                s.backpressure_hits += 1;
            }
            SimEvent::PublishingStopped { total_published } => {
                s.total_published = total_published.max(s.total_published);
                s.publishing_stopped = true;
            }
            SimEvent::SimulationDone => {
                s.sim_done = true;
                s.sim_end = Some(std::time::Instant::now());
                s.freeze_on_done();
                break;
            }
        }
    }
}

/// Periodically samples throughput and latency. Call from a separate spawned task.
pub async fn run_sampler(state: Arc<Mutex<VisualizationState>>, bus: jaeb::EventBus) {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        interval.tick().await;
        {
            let mut s = state.lock().expect("viz state lock poisoned");
            if s.sim_done {
                break;
            }
            s.sample_throughput();
        }
        if let Ok(stats) = bus.stats().await {
            let mut s = state.lock().expect("viz state lock poisoned");
            s.bus_in_flight_async = stats.in_flight_async;
            s.bus_dispatches_in_flight = stats.dispatches_in_flight;
            s.bus_total_subscriptions = stats.total_subscriptions;
        }
    }
}
