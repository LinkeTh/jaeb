//! Dead-letter pipeline: terminal handler failures are routed to a
//! dedicated `DeadLetter` sink via `subscribe_dead_letters`.

use std::time::Duration;

use jaeb::{DeadLetter, EventBus, EventHandler, HandlerResult, RetryStrategy, SubscriptionPolicy, SyncEventHandler};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Payment {
    id: u32,
}

// ── Handlers ────────────────────────────────────────────────────────────

/// Always-failing handler to exhaust retries and trigger a dead letter.
struct OnPayment;

impl EventHandler<Payment> for OnPayment {
    async fn handle(&self, event: &Payment) -> HandlerResult {
        Err(format!("gateway unavailable for payment {}", event.id).into())
    }

    fn name(&self) -> Option<&'static str> {
        Some("payment-handler")
    }
}

/// Receives dead-letter events after all retries are exhausted.
struct DeadLetterSink;

impl SyncEventHandler<DeadLetter> for DeadLetterSink {
    fn handle(&self, dl: &DeadLetter) -> HandlerResult {
        println!(
            "dead letter: event={}, handler={:?}, attempts={}, error={}",
            dl.event_name, dl.handler_name, dl.attempts, dl.error
        );
        // Recover the original payload via downcast.
        if let Some(payment) = dl.event.downcast_ref::<Payment>() {
            println!("  original payload: {:?}", payment);
        }
        Ok(())
    }
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let bus = EventBus::new(64).expect("valid config");

    let policy = SubscriptionPolicy::default()
        .with_max_retries(2)
        .with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(50)))
        .with_dead_letter(true);

    let _ = bus
        .subscribe_with_policy::<Payment, _, _>(OnPayment, policy)
        .await
        .expect("subscribe handler failed");

    // subscribe_dead_letters forces dead_letter=false to prevent recursion.
    let _ = bus
        .subscribe_dead_letters(DeadLetterSink)
        .await
        .expect("subscribe dead-letter sink failed");

    bus.publish(Payment { id: 99 }).await.expect("publish failed");

    // Wait for retries and dead-letter delivery.
    tokio::time::sleep(Duration::from_secs(1)).await;
    bus.shutdown().await.expect("shutdown failed");

    println!("done");
}
