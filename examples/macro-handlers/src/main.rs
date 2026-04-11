//! Demonstrates `#[handler]` and `register_handlers!`.

use std::time::Duration;

use jaeb::{DeadLetter, EventBus, HandlerResult, handler, register_handlers};

#[derive(Clone, Debug)]
struct Payment {
    id: u32,
}

#[handler(retries = 2, retry_strategy = "fixed", retry_base_ms = 50, dead_letter = true, name = "payment-processor")]
async fn process_payment(event: &Payment) -> HandlerResult {
    Err(format!("simulated failure for payment {}", event.id).into())
}

#[handler]
fn log_dead_letter(event: &DeadLetter) -> HandlerResult {
    println!(
        "dead-letter: event={}, listener={:?}, attempts={}, error={}",
        event.event_name, event.listener_name, event.attempts, event.error
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), jaeb::EventBusError> {
    let bus = EventBus::new(64)?;

    register_handlers!(bus, process_payment, log_dead_letter)?;

    bus.publish(Payment { id: 7 }).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;

    bus.shutdown().await
}
