# JAEB - Just Another Event Bus

In-process, actor-based event bus for Tokio applications.

JAEB provides:

- sync + async listeners
- explicit listener unsubscription
- dependency injection via handler structs
- retry policies for failing handlers
- dead-letter stream for terminal failures
- explicit `Result`-based error handling
- graceful shutdown with queue draining

## Installation

```toml
[dependencies]
jaeb = { version = "0.1.0" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## Quick Start

```rust
use std::time::Duration;

use jaeb::{
    DeadLetter, EventBus, EventBusError, EventHandler, FailurePolicy,
    HandlerResult, SyncEventHandler,
};

#[derive(Clone)]
struct OrderCheckoutEvent {
    order_id: i64,
}

struct AsyncCheckoutHandler;

impl EventHandler<OrderCheckoutEvent> for AsyncCheckoutHandler {
    async fn handle(&self, event: &OrderCheckoutEvent) -> HandlerResult {
        println!("async checkout {}", event.order_id);
        Ok(())
    }
}

struct SyncAuditHandler;

impl SyncEventHandler<OrderCheckoutEvent> for SyncAuditHandler {
    fn handle(&self, event: &OrderCheckoutEvent) -> HandlerResult {
        println!("sync audit {}", event.order_id);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), EventBusError> {
    let bus = EventBus::new(64);

    let retry_policy = FailurePolicy::default()
        .with_max_retries(2)
        .with_retry_delay(Duration::from_millis(50));

    let checkout_sub = bus
        .register_with_policy::<OrderCheckoutEvent, _>(AsyncCheckoutHandler, retry_policy)
        .await?;

    let _audit_sub = bus
        .register_sync::<OrderCheckoutEvent, _>(SyncAuditHandler)
        .await?;

    bus.subscribe_dead_letters(|dl: &DeadLetter| {
        eprintln!(
            "dead-letter: event={} listener={} attempts={} error={}",
            dl.event_name, dl.subscription_id, dl.attempts, dl.error
        );
        Ok(())
    })
    .await?;

    bus.publish(OrderCheckoutEvent { order_id: 42 }).await?;
    bus.try_publish(OrderCheckoutEvent { order_id: 43 })?;

    checkout_sub.unsubscribe().await?;
    bus.shutdown().await?;
    Ok(())
}
```

## API Overview

- `subscribe_sync::<E, _>(Fn(&E) -> HandlerResult) -> Result<Subscription, EventBusError>`
- `subscribe_async::<E, _, _>(Fn(E) -> Future<Output = HandlerResult>) -> Result<Subscription, EventBusError>`
- `register::<E, H>(handler) -> Result<Subscription, EventBusError>`
- `register_with_policy::<E, H>(handler, FailurePolicy) -> Result<Subscription, EventBusError>`
- `register_sync::<E, H>(handler) -> Result<Subscription, EventBusError>`
- `publish::<E>(event) -> Result<(), EventBusError>`
- `try_publish::<E>(event) -> Result<(), EventBusError>`
- `unsubscribe(subscription_id) -> Result<bool, EventBusError>`
- `shutdown() -> Result<(), EventBusError>`

### Types

- `HandlerResult = Result<(), Box<dyn Error + Send + Sync>>`
- `FailurePolicy { max_retries, retry_delay, dead_letter }`
- `DeadLetter { event_name, subscription_id, attempts, error }`

## Semantics

- `publish` waits for dispatch and sync listeners to finish.
- `publish` does not wait for async listeners to finish.
- handler failures can be retried based on `FailurePolicy`.
- after retries are exhausted, dead-letter events are emitted when enabled.
- `shutdown` drains queued publish messages and waits for in-flight async listeners.

## Error variants

- `EventBusError::ActorStopped` - actor is shut down
- `EventBusError::ChannelFull` - queue is full (`try_publish` only)

## Notes

- JAEB requires a running Tokio runtime.
- Events are in-process only (no persistence, replay, or broker integration).
