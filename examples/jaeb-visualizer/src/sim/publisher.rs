use std::time::{Duration, Instant};

use jaeb::{EventBus, EventBusError};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::config::types::{PublishConfig, StopCondition};
use crate::sim::events::SimEvent;
use crate::sim::handlers::{SimEventA, SimEventB};

/// Runs the publish loop, sending events at the configured rate.
pub async fn run_publish_loop(bus: EventBus, config: PublishConfig, tx: mpsc::UnboundedSender<SimEvent>) {
    let interval_dur = Duration::from_secs_f64(1.0 / config.events_per_sec);
    let mut ticker = tokio::time::interval(interval_dur);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let start = Instant::now();
    let mut seq: u64 = 0;

    loop {
        ticker.tick().await;
        seq += 1;

        // Check stop condition
        let should_stop = match &config.stop_condition {
            StopCondition::TotalEvents(n) => seq > *n as u64,
            StopCondition::Duration(d) => start.elapsed() >= *d,
        };
        if should_stop {
            break;
        }

        // Publish event (alternate between A and B)
        let published = if seq.is_multiple_of(2) {
            publish_event_a(&bus, seq, &tx)
        } else {
            publish_event_b(&bus, seq, &tx)
        };

        if !published {
            break; // bus stopped
        }

        // Burst: fire extra events
        if config.burst_every_n > 0 && seq.is_multiple_of(config.burst_every_n) {
            for burst_i in 0..5u64 {
                seq += 1;
                let published = if (seq + burst_i).is_multiple_of(2) {
                    publish_event_a(&bus, seq, &tx)
                } else {
                    publish_event_b(&bus, seq, &tx)
                };
                if !published {
                    break;
                }
            }
        }
    }

    let _ = tx.send(SimEvent::SimulationDone);
}

fn publish_event_a(bus: &EventBus, seq: u64, tx: &mpsc::UnboundedSender<SimEvent>) -> bool {
    match bus.try_publish(SimEventA { seq }) {
        Ok(()) => {
            let _ = tx.send(SimEvent::Published {
                event_type: 0,
                seq,
                at: Instant::now(),
            });
            true
        }
        Err(EventBusError::ChannelFull) => {
            let _ = tx.send(SimEvent::BackpressureHit { at: Instant::now() });
            true // bus still alive, just full
        }
        Err(EventBusError::Stopped) => false,
        Err(_) => true,
    }
}

fn publish_event_b(bus: &EventBus, seq: u64, tx: &mpsc::UnboundedSender<SimEvent>) -> bool {
    match bus.try_publish(SimEventB { seq }) {
        Ok(()) => {
            let _ = tx.send(SimEvent::Published {
                event_type: 1,
                seq,
                at: Instant::now(),
            });
            true
        }
        Err(EventBusError::ChannelFull) => {
            let _ = tx.send(SimEvent::BackpressureHit { at: Instant::now() });
            true
        }
        Err(EventBusError::Stopped) => false,
        Err(_) => true,
    }
}
