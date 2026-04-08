# JAEB - Just Another Event Bus

In-process, actor-based event bus for Tokio applications.

JAEB provides:

- sync + async listeners via a unified `subscribe` API
- automatic dispatch-mode selection based on handler trait
- explicit listener unsubscription via `Subscription` handles
- dependency injection via handler structs
- retry policies with configurable delay for failing handlers
- dead-letter stream for terminal failures
- explicit `Result`-based error handling
- graceful shutdown with queue draining and in-flight task completion
- optional Prometheus-compatible metrics via the `metrics` crate
- structured tracing with per-handler spans
- `#![forbid(unsafe_code)]`

## Installation

```toml
[dependencies]
jaeb = { version = "0.2.0" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

To enable metrics instrumentation:

```toml
[dependencies]
jaeb = { version = "0.2.0", features = ["metrics"] }
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

struct DeadLetterLogger;

impl SyncEventHandler<DeadLetter> for DeadLetterLogger {
    fn handle(&self, dl: &DeadLetter) -> HandlerResult {
        eprintln!(
            "dead-letter: event={} listener={} attempts={} error={}",
            dl.event_name, dl.subscription_id, dl.attempts, dl.error
        );
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), EventBusError> {
    let bus = EventBus::new(64);

    let retry_policy = FailurePolicy::default()
        .with_max_retries(2)
        .with_retry_delay(Duration::from_millis(50));

    // Async handler -- dispatch mode inferred from EventHandler impl
    let checkout_sub = bus
        .subscribe_with_policy(AsyncCheckoutHandler, retry_policy)
        .await?;

    // Sync handler -- dispatch mode inferred from SyncEventHandler impl
    let _audit_sub = bus.subscribe(SyncAuditHandler).await?;

    // Dead-letter handler (convenience method, auto-sets dead_letter: false)
    bus.subscribe_dead_letters(DeadLetterLogger).await?;

    bus.publish(OrderCheckoutEvent { order_id: 42 }).await?;
    bus.try_publish(OrderCheckoutEvent { order_id: 43 })?;

    checkout_sub.unsubscribe().await?;
    bus.shutdown().await?;
    Ok(())
}
```

## API Overview

### EventBus

- `EventBus::new(buffer)` -- create a new bus with the given channel capacity
- `subscribe(handler) -> Result<Subscription, EventBusError>` -- dispatch mode inferred from trait
- `subscribe_with_policy(handler, FailurePolicy) -> Result<Subscription, EventBusError>`
- `subscribe_dead_letters(handler) -> Result<Subscription, EventBusError>`
- `publish(event) -> Result<(), EventBusError>`
- `try_publish(event) -> Result<(), EventBusError>` -- non-blocking, returns `ChannelFull` when queue is full
- `unsubscribe(subscription_id) -> Result<bool, EventBusError>`
- `shutdown() -> Result<(), EventBusError>`

`EventBus` is `Clone` -- all clones share the same underlying actor.

### Handler Traits

- `EventHandler<E>` -- async handler, dispatched on a spawned task (requires `E: Clone`)
- `SyncEventHandler<E>` -- sync handler, awaited inline during dispatch

The dispatch mode is selected automatically based on which trait is implemented.
The `IntoHandler<E, Mode>` trait performs the conversion; the `Mode` parameter
(`AsyncMode` / `SyncMode`) is inferred, so callers simply write `bus.subscribe(handler)`.

### Subscription

`Subscription` holds a `SubscriptionId` and a bus handle. Call `subscription.unsubscribe()`
to remove the handler, or use `bus.unsubscribe(id)` directly.

### Types

- `Event` -- blanket trait implemented for all `T: Send + Sync + 'static`
- `HandlerResult = Result<(), Box<dyn Error + Send + Sync>>`
- `FailurePolicy { max_retries, retry_delay, dead_letter }`
- `DeadLetter { event_name, subscription_id, attempts, error }`
- `SubscriptionId` -- opaque handler ID (wraps `u64`)

## Feature Flags

| Flag | Default | Description |
|------|---------|-------------|
| `metrics` | off | Enables Prometheus-compatible instrumentation via the `metrics` crate |

When the `metrics` feature is enabled, the bus records:

- `eventbus.publish` (counter, per event type)
- `eventbus.handler.duration` (histogram, per event type)
- `eventbus.handler.error` (counter, per event type)
- `eventbus.handler.join_error` (counter, per event type)

## Observability

JAEB uses the `tracing` crate throughout. Key spans and events:

- `event_bus.publish` / `event_bus.subscribe` / `event_bus.shutdown` -- top-level operations
- `eventbus.handler` span -- per-handler execution with `event`, `mode`, and `listener_id` fields
- `handler.retry` (warn) -- logged on each retry attempt
- `handler.failed` (error) -- logged when retries are exhausted
- `handler.join_error` (error) -- logged when a spawned task panics

## Semantics

- `publish` waits for dispatch and sync listeners to finish.
- `publish` does **not** wait for async listeners to finish.
- handler failures can be retried based on `FailurePolicy`.
- after retries are exhausted, dead-letter events are emitted when `dead_letter: true`.
- dead-letter handlers themselves cannot trigger further dead letters (recursion guard).
- `shutdown` drains queued publish messages and waits for in-flight async listeners.
- after shutdown, all operations return `EventBusError::ActorStopped`.

## Error Variants

- `EventBusError::ActorStopped` -- the actor has shut down or the channel is closed
- `EventBusError::ChannelFull` -- the internal queue is full (`try_publish` only)

## Examples

See [`examples/jaeb-demo`](examples/jaeb-demo) for a working demo that includes:

- async and sync handlers with retry policies
- dead-letter logging
- Prometheus metrics exporter
- structured tracing setup

Run it with:

```sh
cd examples/jaeb-demo
RUST_LOG=info,jaeb=trace cargo run
```

## Notes

- JAEB requires a running Tokio runtime.
- Events must be `Send + Sync + 'static`. Async handlers additionally require events to be `Clone`.
- Events are in-process only (no persistence, replay, or broker integration).
- The crate enforces `#![forbid(unsafe_code)]`.

## License

jaeb is distributed under the [MIT License](LICENCE).

Copyright (c) 2025-2026 Linke Thomas

This project uses third-party libraries. See [THIRD-PARTY-LICENSES](THIRD-PARTY-LICENSES)
for the full list of dependencies, their versions, and their respective license terms.
