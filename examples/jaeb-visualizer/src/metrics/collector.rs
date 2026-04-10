use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::metrics::state::{DeadLetterEntry, FlowBlip, VisualizationState};
use crate::sim::SimEvent;

/// Drains SimEvent from the channel and updates VisualizationState.
pub async fn run_collector(mut rx: mpsc::UnboundedReceiver<SimEvent>, state: Arc<Mutex<VisualizationState>>, event_type_names: [String; 2]) {
    while let Some(event) = rx.recv().await {
        let mut s = state.lock().expect("viz state lock poisoned");
        match event {
            SimEvent::Published { event_type, seq, at } => {
                s.total_published += 1;
                s.in_flight += 1;
                s.add_flow_blip(FlowBlip {
                    event_type,
                    progress: 0.0,
                    spawned_at: at,
                    ttl_secs: 1.5,
                });
                let _ = (seq,); // used for flow tracking
            }
            SimEvent::HandlerStarted { .. } => {}
            SimEvent::HandlerCompleted { .. } => {
                s.total_handled += 1;
                s.in_flight -= 1;
            }
            SimEvent::HandlerFailed { .. } => {
                s.total_failed += 1;
            }
            SimEvent::DeadLetterReceived {
                listener,
                event_type,
                seq,
                error,
                at,
            } => {
                s.total_dead_letters += 1;
                s.in_flight -= 1;
                let elapsed = s.sim_start.map(|start| at.duration_since(start).as_secs_f64()).unwrap_or(0.0);
                let type_name = event_type_names
                    .get(event_type as usize)
                    .cloned()
                    .unwrap_or_else(|| format!("Type{}", event_type));
                s.add_dead_letter(DeadLetterEntry {
                    at,
                    elapsed_secs: elapsed,
                    listener,
                    event_type: type_name,
                    seq,
                    reason: error,
                });
            }
            SimEvent::BackpressureHit { .. } => {
                s.backpressure_hits += 1;
            }
            SimEvent::SimulationDone => {
                s.sim_done = true;
                break;
            }
        }
    }
}

/// Periodically samples throughput and prunes old data. Call from a separate spawned task.
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
        // Poll bus stats
        if let Ok(stats) = bus.stats().await {
            let mut s = state.lock().expect("viz state lock poisoned");
            s.bus_in_flight_async = stats.in_flight_async;
            s.bus_total_subscriptions = stats.total_subscriptions;
        }
    }
}
