//! Continuous simulation loop that publishes a mixed stream of events.
//!
//! With jaeb's built-in span propagation, we just need to enter the span
//! before calling `publish()` — the bus captures `Span::current()` and
//! instruments spawned async handler futures automatically.

use std::time::Duration;

use jaeb::EventBus;
use rand::Rng;
use tokio::sync::watch;
use tracing::{Instrument, info, info_span};
use uuid::Uuid;

use crate::events::*;

static CARRIERS: &[&str] = &["FedEx", "UPS", "DHL", "USPS"];

/// Runs until `stop_rx` fires, publishing a mixed stream of events every
/// `tick_ms` +/- 50% jitter milliseconds.
pub async fn run_loop(bus: EventBus, mut stop_rx: watch::Receiver<bool>, tick_ms: u64) {
    let mut rng = rand::rng();
    let mut iteration: u64 = 0;

    loop {
        // Respect shutdown signal (non-blocking check).
        if *stop_rx.borrow() {
            break;
        }

        iteration += 1;
        let order_id = Uuid::new_v4().to_string();

        // Create a parent span for the entire order flow -- all events in this
        // tick share it so Tempo shows a single trace per order.
        let flow_span = info_span!("order.flow", %order_id, iteration);

        // Run the publish calls inside the flow span — jaeb captures it
        // automatically and instruments async handlers with it.
        async {
            // Always publish OrderCreated.
            let _ = bus
                .publish(OrderCreated {
                    order_id: order_id.clone(),
                    amount_cents: rng.random_range(100..50_000),
                })
                .await;

            // 80% of orders proceed to payment.
            if rng.random_bool(0.80) {
                let success = rng.random_bool(0.75); // 75% payment success
                let _ = bus
                    .publish(PaymentProcessed {
                        order_id: order_id.clone(),
                        success,
                    })
                    .await;

                // Only successful payments get shipped.
                if success {
                    let carrier = CARRIERS[rng.random_range(0..CARRIERS.len())];
                    let _ = bus
                        .publish(ShipmentDispatched {
                            order_id: order_id.clone(),
                            carrier,
                        })
                        .await;
                }
            }

            // ~10% of orders trigger fraud alerts (always dead-lettered).
            if rng.random_bool(0.10) {
                let _ = bus
                    .publish(FraudCheckFailed {
                        order_id: order_id.clone(),
                        reason: "velocity check exceeded".to_string(),
                    })
                    .await;
            }

            info!(iteration, order_id = %order_id, "simulation tick complete");
        }
        .instrument(flow_span)
        .await;

        // Jittered sleep: tick_ms +/- 50%.
        let half = tick_ms / 2;
        let jitter = rng.random_range(half..tick_ms + half);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(jitter)) => {}
            _ = stop_rx.changed() => { break; }
        }
    }

    info!("simulation loop exiting");
}
