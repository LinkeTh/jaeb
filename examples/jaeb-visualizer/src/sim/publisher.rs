use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use jaeb::EventBus;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::config::types::{EventTypeConfig, PublishConfig, PublishPattern, StopCondition};
use crate::sim::events::SimEvent;
use crate::sim::handlers::SimEnvelopeEvent;

/// Runs the publish loop, sending events at configured per-event-type rates.
/// Returns total published events.
pub async fn run_publish_loop(
    bus: EventBus,
    config: PublishConfig,
    event_types: Vec<EventTypeConfig>,
    processed_counter: Arc<AtomicU64>,
    tx: mpsc::UnboundedSender<SimEvent>,
) -> u64 {
    if event_types.is_empty() {
        return 0;
    }

    let min_rate = event_types.iter().map(|t| t.events_per_sec).fold(f64::INFINITY, f64::min).max(0.1);
    let interval_dur = Duration::from_secs_f64(1.0 / min_rate);
    let mut ticker = tokio::time::interval(interval_dur);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let start = Instant::now();
    let mut seq: u64 = 0;
    let mut total_published: u64 = 0;

    let mut event_type_counters = vec![0u64; event_types.len()];
    let mut accumulators = vec![0.0f64; event_types.len()];
    let tick_secs = interval_dur.as_secs_f64();

    loop {
        ticker.tick().await;

        match &config.stop_condition {
            StopCondition::Duration(d) if start.elapsed() >= *d => break,
            StopCondition::TotalEvents(target) if processed_counter.load(Ordering::Relaxed) >= *target as u64 => break,
            _ => {}
        }

        for (idx, event_type) in event_types.iter().enumerate() {
            accumulators[idx] += event_type.events_per_sec * tick_secs;
            while accumulators[idx] >= 1.0 {
                accumulators[idx] -= 1.0;

                seq += 1;
                event_type_counters[idx] += 1;
                if publish_event(&bus, idx, seq, &tx).await {
                    total_published += 1;
                }

                match event_type.pattern {
                    PublishPattern::Burst => {
                        let n = event_type.burst_every_n.max(1);
                        if event_type_counters[idx].is_multiple_of(n) {
                            for _ in 0..5 {
                                seq += 1;
                                event_type_counters[idx] += 1;
                                if publish_event(&bus, idx, seq, &tx).await {
                                    total_published += 1;
                                }
                            }
                        }
                    }
                    PublishPattern::Random => {
                        if rand::random::<f64>() < 0.25 {
                            seq += 1;
                            event_type_counters[idx] += 1;
                            if publish_event(&bus, idx, seq, &tx).await {
                                total_published += 1;
                            }
                        }
                    }
                    PublishPattern::Constant => {}
                }
            }
        }
    }

    total_published
}

async fn publish_event(bus: &EventBus, event_type_idx: usize, seq: u64, tx: &mpsc::UnboundedSender<SimEvent>) -> bool {
    let published_at = Instant::now();
    let res = bus
        .publish(SimEnvelopeEvent {
            seq,
            event_type_idx,
            published_at,
        })
        .await;

    match res {
        Ok(()) => {
            let _ = tx.send(SimEvent::Published {
                event_type_idx,
                seq,
                at: Instant::now(),
            });
            true
        }
        Err(jaeb::EventBusError::Stopped) => false,
        Err(_) => true,
    }
}
