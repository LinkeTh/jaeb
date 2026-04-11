# JAEB - Just Another Event Bus

[![crates.io](https://img.shields.io/crates/v/jaeb.svg)](https://crates.io/crates/jaeb)
[![docs.rs](https://docs.rs/jaeb/badge.svg)](https://docs.rs/jaeb)
[![CI](https://github.com/LinkeTh/jaeb/actions/workflows/ci.yml/badge.svg)](https://github.com/LinkeTh/jaeb/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/MSRV-1.94-blue.svg)](https://github.com/LinkeTh/jaeb)
[![license](https://img.shields.io/crates/l/jaeb.svg)](https://github.com/LinkeTh/jaeb/blob/main/LICENSE)

In-process, snapshot-driven event bus for Tokio applications.

JAEB focuses on correctness and observability for monolith-style event-driven Rust services:

- sync + async handlers behind one `subscribe` API
- compile-time policy validation (retry policies cannot be used with sync handlers)
- listener priority with FIFO stability for equal priorities
- typed and global middleware
- dead-letter stream with recursion guard
- graceful shutdown with in-flight async drain
- optional metrics (`metrics` feature) and built-in tracing
- optional standalone macros (`macros` feature): `#[handler]` and `register_handlers!`
- [summer-rs](https://crates.io/crates/summer) integration via [summer-jaeb](./summer-jaeb) and `#[event_listener]` macro
  support [summer-jaeb-macros](./summer-jaeb-macros)

## When to use JAEB

Use JAEB when you need:

- domain events inside one process (e.g. `OrderCreated` -> projections, notifications, audit)
- decoupled modules with type-safe fan-out
- retry/dead-letter behavior per listener
- deterministic sync-lane ordering with priority hints

JAEB is not a message broker. It does **not** provide persistence, replay, or cross-process delivery.

## Installation

```toml
[dependencies]
jaeb = "0.3.5"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

With metrics instrumentation:

```toml
[dependencies]
jaeb = { version = "0.3.5", features = ["metrics"] }
```

With standalone handler macros:

```toml
[dependencies]
jaeb = { version = "0.3.5", features = ["macros"] }
```

## Quick Start

```rust
use std::time::Duration;

use jaeb::{
    DeadLetter, EventBus, EventBusError, EventHandler, HandlerResult, RetryStrategy, SubscriptionPolicy, SyncEventHandler,
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
    let bus = EventBus::new(64)?;

    let retry_policy = SubscriptionPolicy::default()
        .with_priority(10)
        .with_max_retries(2)
        .with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(50)));

    let checkout_sub = bus
        .subscribe_with_policy::<OrderCheckoutEvent, _, _>(AsyncCheckoutHandler, retry_policy)
        .await?;

    let _audit_sub = bus.subscribe::<OrderCheckoutEvent, _, _>(SyncAuditHandler).await?;
    let _dl_sub = bus.subscribe_dead_letters(DeadLetterLogger).await?;

    bus.publish(OrderCheckoutEvent { order_id: 42 }).await?;
    bus.try_publish(OrderCheckoutEvent { order_id: 43 })?;

    checkout_sub.unsubscribe().await?;
    bus.shutdown().await?;
    Ok(())
}
```

## Architecture

JAEB uses an immutable snapshot registry (`ArcSwap`) for hot-path reads:

```text
publish(event)
  -> load snapshot (lock-free)
  -> global middleware
  -> typed middleware
  -> async lane (spawned)
  -> sync lane (serialized FIFO, priority-ordered)
```

- async and sync listeners are separated per event type
- priority is applied per lane (higher first)
- equal priority preserves registration order

## API Highlights

- `EventBus::builder()` for buffer size, timeouts, concurrency limit, and default policy
- `default_subscription_policy(SubscriptionPolicy)` sets fallback policy for `subscribe`
- `subscribe_with_policy(handler, policy)` accepts:
    - `SubscriptionPolicy` for async handlers
    - `SyncSubscriptionPolicy` for sync handlers and once handlers
- `publish` waits for sync listeners and task-spawn for async listeners
- `try_publish` is non-blocking and returns `EventBusError::ChannelFull` on saturation

Core policy types:

- `SubscriptionPolicy { priority, max_retries, retry_strategy, dead_letter }`
- `SyncSubscriptionPolicy { priority, dead_letter }`
- `IntoSubscriptionPolicy<M>` sealed trait for compile-time mode/policy safety

Backward-compatible aliases remain available (deprecated):

- `FailurePolicy` -> `SubscriptionPolicy`
- `NoRetryPolicy` -> `SyncSubscriptionPolicy`
- `IntoFailurePolicy` -> `IntoSubscriptionPolicy`

## Performance

See [`BENCHMARK.md`](BENCHMARK.md) for:

- cross-library benchmark setup (`jaeb` vs `eventbuzz` vs `evno`)
- reproducible benchmark command
- measured results and caveats
- documented `evno` contention benchmark hang under this environment

## Examples

- `examples/basic-pubsub` - minimal publish/subscribe
- `examples/sync-handler` - sync dispatch lane behavior
- `examples/closure-handlers` - closure-based handlers
- `examples/retry-strategies` - fixed/exponential/jitter retry configuration
- `examples/dead-letters` - dead-letter subscription and inspection
- `examples/middleware` - global and typed middleware
- `examples/backpressure` - `try_publish` saturation behavior
- `examples/concurrency-limit` - max concurrent async handlers
- `examples/graceful-shutdown` - controlled shutdown and draining
- `examples/introspection` - `EventBus::stats()` output
- `examples/axum-integration` - axum REST app publishing domain events
- `examples/macro-handlers` - standalone `#[handler]` + `register_handlers!`
- `examples/macro-handlers-auto` - standalone `#[handler]` auto-discovery with `register_handlers!(bus)`
- `examples/jaeb-demo` - full demo with tracing + metrics exporter
- `examples/summer-jaeb-demo` - summer-rs plugin + `#[event_listener]`

Run an example:

```sh
cargo run -p axum-integration
```

## Feature Flags

| Flag         | Default | Description                                                 |
|--------------|---------|-------------------------------------------------------------|
| `macros`     | off     | Re-exports `#[handler]` and `register_handlers!`            |
| `metrics`    | off     | Enables Prometheus-compatible instrumentation via `metrics` |
| `test-utils` | off     | Exposes `TestBus` helpers for integration tests             |

When `metrics` is enabled, JAEB records:

- `eventbus.publish` (counter, per event type)
- `eventbus.handler.duration` (histogram, per event type)
- `eventbus.handler.error` (counter, per event type)
- `eventbus.handler.join_error` (counter, per event type)

## summer-rs Integration

Use [`summer-jaeb`](summer-jaeb) and [`summer-jaeb-macros`](summer-jaeb-macros) for plugin-based auto-registration via `#[event_listener]`.

Macro support includes:

- `retries`
- `retry_strategy`
- `retry_base_ms`
- `retry_max_ms`
- `dead_letter`
- `priority`
- `name`

## Standalone Macros

Enable the `macros` feature to use `#[handler]` and `register_handlers!` without
summer-rs.

The `#[handler]` macro generates a struct named `<FunctionName>Handler` and an
async `register(&EventBus)` method. Policy attributes are supported:

- `retries`
- `retry_strategy`
- `retry_base_ms`
- `retry_max_ms`
- `dead_letter`
- `priority`
- `name`

## Notes

- JAEB requires a running Tokio runtime.
- Events must be `Send + Sync + 'static`; async handlers also require `Clone`.
- The crate enforces `#![forbid(unsafe_code)]`.

## License

jaeb is distributed under the [MIT License](https://github.com/LinkeTh/jaeb/blob/main/LICENSE).

Copyright (c) 2025-2026 Linke Thomas

This project uses third-party libraries. See [THIRD-PARTY-LICENSES](THIRD-PARTY-LICENSES)
for dependency and license details.
