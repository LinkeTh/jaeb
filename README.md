# ⚡ JAEB — Just Another Event Bus

[![crates.io](https://img.shields.io/crates/v/jaeb.svg)](https://crates.io/crates/jaeb)
[![docs.rs](https://docs.rs/jaeb/badge.svg)](https://docs.rs/jaeb)
[![CI](https://github.com/LinkeTh/jaeb/actions/workflows/ci.yml/badge.svg)](https://github.com/LinkeTh/jaeb/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/MSRV-1.94-blue.svg)](https://github.com/LinkeTh/jaeb)
[![license](https://img.shields.io/crates/l/jaeb.svg)](https://github.com/LinkeTh/jaeb/blob/main/LICENSE)
[![downloads](https://img.shields.io/crates/d/jaeb.svg)](https://crates.io/crates/jaeb)

> A lightweight, in-process event bus for Tokio — snapshot-driven dispatch with retry, dead-letter, and middleware support.

### ✨ Highlights

- 🔀 **Sync + Async** handlers behind one `subscribe` API
- 🔁 **Retry & Dead Letters** with per-listener policies
- 🧩 **Typed & Global Middleware** pipeline
- 📊 **Optional Metrics** (Prometheus-compatible via `metrics` crate)
- 🔍 **Built-in Tracing** support (`trace` feature)
- 🛑 **Graceful Shutdown** with async drain + timeout
- 🏗️ **summer-rs Integration** for plugin-based auto-registration

## When to use JAEB

Use JAEB when you need:

- domain events inside one process (e.g. `OrderCreated` -> projections, notifications, audit)
- decoupled modules with type-safe fan-out
- retry/dead-letter behavior per listener
- deterministic sync-lane ordering with priority hints

JAEB is **not** a message broker — it does not provide persistence, replay, or cross-process delivery.
If you need durable messaging, consider pairing JAEB with an external queue for outbox-style patterns.

## Installation

```toml
[dependencies]
jaeb = "0.5.0"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

With metrics instrumentation:

```toml
[dependencies]
jaeb = { version = "0.5.0", features = ["metrics"] }
```

![png](grafana.png 'grafana dashboard')

With tracing:

```toml
[dependencies]
jaeb = { version = "0.5.0", features = ["trace"] }
```

With standalone handler macros:

```toml
[dependencies]
jaeb = { version = "0.5.0", features = ["macros"] }
```

## ⚡ Quick Start

Full example with sync/async handlers, retry policies, and dead-letter handling:

```rust,ignore
use std::time::Duration;

use jaeb::{
    AsyncSubscriptionPolicy, DeadLetter, EventBus, EventBusError, EventHandler, HandlerResult, RetryStrategy, SyncEventHandler,
};

struct OrderCheckoutEvent {
    order_id: i64,
}

struct AsyncCheckoutHandler;

impl EventHandler<OrderCheckoutEvent> for AsyncCheckoutHandler {
    async fn handle(&self, event: &OrderCheckoutEvent, _bus: &EventBus) -> HandlerResult {
        println!("async checkout {}", event.order_id);
        Ok(())
    }
}

struct SyncAuditHandler;

impl SyncEventHandler<OrderCheckoutEvent> for SyncAuditHandler {
    fn handle(&self, event: &OrderCheckoutEvent, _bus: &EventBus) -> HandlerResult {
        println!("sync audit {}", event.order_id);
        Ok(())
    }
}

struct DeadLetterLogger;

impl SyncEventHandler<DeadLetter> for DeadLetterLogger {
    fn handle(&self, dl: &DeadLetter, _bus: &EventBus) -> HandlerResult {
        eprintln!(
            "dead-letter: event={} listener={} attempts={} error={}",
            dl.event_name, dl.subscription_id, dl.attempts, dl.error
        );
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), EventBusError> {
    let bus = EventBus::builder().build().await?;

    let retry_policy = AsyncSubscriptionPolicy::default()
        .with_max_retries(2)
        .with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(50)));

    let checkout_sub = bus
        .subscribe_with_policy::<OrderCheckoutEvent, _, _>(AsyncCheckoutHandler, retry_policy)
        .await?;

    let _audit_sub = bus.subscribe::<OrderCheckoutEvent, _, _>(SyncAuditHandler).await?;
    let _dl_sub = bus.subscribe_dead_letters(DeadLetterLogger).await?;

    bus.publish(OrderCheckoutEvent { order_id: 42 }).await?;

    checkout_sub.unsubscribe().await?;
    bus.shutdown().await?;
    Ok(())
}
```

Detailed usage patterns (builder descriptors, direct `subscribe*`, manual descriptors,
`Deps`/`Dep<T>`, and trade-offs) are documented in [`USAGE.md`](USAGE.md).

## Architecture

JAEB uses an immutable snapshot registry (`ArcSwap`) for hot-path reads:

```mermaid
graph LR
    P[publish] --> S[Load Snapshot]
    S --> GM[Global Middleware]
    GM --> TM[Typed Middleware]
    TM --> AL[Async Lane - enqueued]
    TM --> SL[Sync Lane - serialized FIFO]
```

- async and sync listeners are separated per event type
- sync priority is applied within the sync lane (higher first)
- equal priority preserves registration order

## API Highlights

- `EventBus::builder()` for timeouts, concurrency limit, and default subscription policies
- `default_subscription_policies(SubscriptionDefaults)` sets fallback policies for `subscribe`
- `subscribe_with_policy(handler, policy)` accepts:
    - `AsyncSubscriptionPolicy` for async handlers
    - `SyncSubscriptionPolicy` for sync handlers and once handlers
- `publish` waits for sync listeners and async-listener task spawning
- `max_concurrent_async(n)` caps async handler execution **across the whole bus**, not per event type; async listeners are scheduled in registration
  order within a publish, then run asynchronously as tracked tasks

Core policy types:

- `AsyncSubscriptionPolicy { max_retries, retry_strategy, dead_letter }`
- `SyncSubscriptionPolicy { priority, dead_letter }`
- `SubscriptionDefaults { policy, sync_policy }`

## Examples

- `examples/basic-pubsub` - minimal publish/subscribe
- `examples/sync-handler` - sync dispatch lane behavior
- `examples/closure-handlers` - closure-based handlers
- `examples/retry-strategies` - fixed/exponential/jitter retry configuration
- `examples/dead-letters` - dead-letter subscription and inspection
- `examples/middleware` - global and typed middleware
- `examples/concurrency-limit` - max concurrent async handlers
- `examples/graceful-shutdown` - controlled shutdown and draining
- `examples/introspection` - `EventBus::stats()` output
- `examples/fire-once` - one-shot / fire-once handler
- `examples/panic-safety` - panic handling behavior in handlers
- `examples/subscription-lifecycle` - subscribe/unsubscribe lifecycle
- `examples/axum-integration` - axum REST app publishing domain events
- `examples/axum-integration-macros` - axum REST app using `#[handler]` + `Dep<T>` DI
- `examples/macro-handlers` - standalone `#[handler]` + `#[dead_letter_handler]` with builder
- `examples/jaeb-demo` - full demo with tracing + metrics exporter
- `examples/summer-jaeb-demo` - summer-rs plugin + `#[event_listener]`
- `examples/observability-stack` - Grafana + Prometheus + Loki + Tempo demo
- `examples/jaeb-visualizer` - TUI visualizer for event bus activity

![png](visualizer.png 'ratatui simulator')

Run an example:

```sh
cargo run -p axum-integration
```

## Feature Flags

| Flag         | Default | Description                                                 |
|--------------|---------|-------------------------------------------------------------|
| `macros`     | off     | Re-exports `#[handler]` and `#[dead_letter_handler]`        |
| `metrics`    | off     | Enables Prometheus-compatible instrumentation via `metrics` |
| `trace`      | off     | Enables `tracing` spans and events for dispatch diagnostics |
| `test-utils` | off     | Exposes `TestBus` helpers for integration tests             |

When `metrics` is enabled, JAEB records:

- `eventbus.publish` (counter, labels: `event`)
- `eventbus.handler.duration` (histogram, labels: `event`, `handler`)
- `eventbus.handler.error` (counter, labels: `event`, `listener`)
- `eventbus.dead_letter` (counter, labels: `event`, `handler`) — fires when a dead letter is created

## summer-rs Integration

Use [`summer-jaeb`](summer-jaeb) and [`summer-jaeb-macros`](summer-jaeb-macros) for plugin-based auto-registration via `#[event_listener]`.

Macro support includes:

- `retries`
- `retry_strategy`
- `retry_base_ms`
- `retry_max_ms`
- `dead_letter`
- `priority` (sync listeners only)
- `name`

```rust,ignore
use jaeb::{DeadLetter, EventBus, HandlerResult};
use summer::{App, AppBuilder, async_trait};
use summer::extractor::Component;
use summer::plugin::{MutableComponentRegistry, Plugin};
use summer_jaeb::{SummerJaeb, event_listener};

#[derive(Debug)]
struct OrderPlacedEvent {
    order_id: u32,
}

/// A dummy database pool registered as a summer Component via a plugin.
#[derive(Clone, Debug)]
struct DbPool;

impl DbPool {
    fn log_order(&self, order_id: u32) {
        println!("DbPool: persisted order {order_id}");
    }
}

struct DbPoolPlugin;

#[async_trait]
impl Plugin for DbPoolPlugin {
    async fn build(&self, app: &mut AppBuilder) {
        app.add_component(DbPool);
    }
    fn name(&self) -> &str { "DbPoolPlugin" }
}

/// Async listener — `DbPool` is injected automatically from summer's DI container.
#[event_listener(retries = 2, retry_strategy = "fixed", retry_base_ms = 500, dead_letter = true)]
async fn on_order_placed(event: &OrderPlacedEvent, Component(db): Component<DbPool>) -> HandlerResult {
    db.log_order(event.order_id);
    Ok(())
}

/// Sync dead-letter listener — auto-detected from the `DeadLetter` event type.
#[event_listener(name = "dead_letter")]
fn on_dead_letter(event: &DeadLetter) -> HandlerResult {
    eprintln!("dead letter: event={}, attempts={}", event.event_name, event.attempts);
    Ok(())
}

#[tokio::main]
async fn main() {
    App::new()
        .add_plugin(DbPoolPlugin)
        .add_plugin(SummerJaeb::new().with_dependency("DbPoolPlugin"))
        .run()
        .await;
}
```

All `#[event_listener]` functions are auto-discovered via `inventory` and subscribed
during plugin startup — no manual registration needed.

## Standalone Macros

Enable the `macros` feature to use `#[handler]` and `#[dead_letter_handler]`
without summer-rs.

`#[handler]` generates a struct named `<FunctionName>Handler` that implements
`HandlerDescriptor`. Register it via `EventBusBuilder::handler`. Policy
attributes are supported:

- `retries`
- `retry_strategy`
- `retry_base_ms`
- `retry_max_ms`
- `dead_letter`
- `priority`
- `name`

`#[dead_letter_handler]` generates a struct that implements
`DeadLetterDescriptor`. The function must be synchronous and accept `&DeadLetter`.
Register it via `EventBusBuilder::dead_letter`.

`Dep<T>` parameters are supported for both macros and are resolved from
`EventBusBuilder::deps(...)` at build time. Supported forms:

- `Dep(name): Dep<T>`
- `name: Dep<T>`

```rust,ignore
use std::time::Duration;
use jaeb::{DeadLetter, EventBus, HandlerResult, dead_letter_handler, handler};

#[derive(Debug)]
struct Payment {
    id: u32,
}

#[handler(retries = 2, retry_strategy = "fixed", retry_base_ms = 50, dead_letter = true, name = "payment-processor")]
async fn process_payment(event: &Payment) -> HandlerResult {
    println!("processing payment {}", event.id);
    Ok(())
}

#[dead_letter_handler]
fn on_dead_letter(event: &DeadLetter) -> HandlerResult {
    println!(
        "dead-letter: event={}, handler={:?}, attempts={}, error={}",
        event.event_name, event.handler_name, event.attempts, event.error
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), jaeb::EventBusError> {
    let bus = EventBus::builder()
        
        .handler(process_payment)
        .dead_letter(on_dead_letter)
        .build()
        .await?;
    bus.publish(Payment { id: 7 }).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    bus.shutdown().await
}
```

`Dep<T>` example:

```rust,ignore
use std::sync::Arc;
use jaeb::{Dep, Deps, EventBus, HandlerResult, handler};

struct AuditLog;

struct Payment;

#[handler]
async fn process_with_dep(_event: &Payment, Dep(log): Dep<Arc<AuditLog>>) -> HandlerResult {
    let _ = log;
    Ok(())
}

#[handler]
fn process_with_wrapper(_event: &Payment, log: Dep<Arc<AuditLog>>) -> HandlerResult {
    let _inner: Arc<AuditLog> = log.0;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), jaeb::EventBusError> {
    let log = Arc::new(AuditLog);
    let bus = EventBus::builder()
        .handler(process_with_dep)
        .handler(process_with_wrapper)
        .deps(Deps::new().insert(log))
        .build()
        .await?;
    bus.shutdown().await
}
```

## Notes

- JAEB requires a running Tokio runtime.
- Events published through JAEB must be `Send + Sync + 'static`.
- The crate enforces `#![forbid(unsafe_code)]`.

## License

jaeb is distributed under the [MIT License](https://github.com/LinkeTh/jaeb/blob/main/LICENSE).

Copyright (c) 2025-2026 Linke Thomas

This project uses third-party libraries. See [THIRD-PARTY-LICENSES](THIRD-PARTY-LICENSES)
for dependency and license details.
