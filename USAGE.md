# Usage Guide

This guide explains the supported subscription and dependency patterns in `jaeb`
0.4+.

## Subscription Paths

There are three valid ways to subscribe handlers.

### 1) Macro + Builder descriptors

Use `#[handler]` / `#[dead_letter_handler]`, then register via
`EventBus::builder()`.

```rust,ignore
use jaeb::{DeadLetter, EventBus, HandlerResult, dead_letter_handler, handler};

#[derive(Clone)]
struct OrderPlaced;

#[handler]
async fn on_order(_event: &OrderPlaced) -> HandlerResult {
    Ok(())
}

#[dead_letter_handler]
fn on_dead_letter(_event: &DeadLetter) -> HandlerResult {
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), jaeb::EventBusError> {
    let bus = EventBus::builder()
        .handler(on_order)
        .dead_letter(on_dead_letter)
        .build()
        .await?;

    bus.shutdown().await
}
```

When to use it:

- You want concise handler declarations.
- You want builder-time dependency injection with `Deps`.
- You want policy attributes (`retries`, `priority`, etc.) on handlers.

### 2) Direct runtime subscription (`bus.subscribe*`)

Build a bus, then subscribe handlers directly with `subscribe`,
`subscribe_with_policy`, and `subscribe_dead_letters`.

```rust,ignore
use std::time::Duration;
use jaeb::{EventBus, EventHandler, HandlerResult, RetryStrategy, SubscriptionPolicy};

#[derive(Clone)]
struct OrderPlaced;

struct EmailHandler;

impl EventHandler<OrderPlaced> for EmailHandler {
    async fn handle(&self, _event: &OrderPlaced) -> HandlerResult {
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), jaeb::EventBusError> {
    let bus = EventBus::builder().build().await?;

    let policy = SubscriptionPolicy::default()
        .with_max_retries(2)
        .with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(50)));

    let _sub = bus
        .subscribe_with_policy::<OrderPlaced, _, _>(EmailHandler, policy)
        .await?;

    // closures also work:
    let _sync_sub = bus
        .subscribe::<OrderPlaced, _, _>(|_event: &OrderPlaced| Ok(()))
        .await?;

    bus.shutdown().await
}
```

When to use it:

- You want dynamic/conditional registration after startup.
- You construct handler instances yourself.
- You do not need builder descriptors.

### 3) Manual descriptor + Builder

Implement `HandlerDescriptor` / `DeadLetterDescriptor` yourself and pass the
descriptor to the builder.

```rust,ignore
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use jaeb::{Deps, EventBus, EventBusError, EventHandler, HandlerDescriptor, HandlerResult, Subscription};

#[derive(Clone)]
struct OrderPlaced;

#[derive(Clone)]
struct Db;

struct DbHandler {
    db: Arc<Db>,
}

impl EventHandler<OrderPlaced> for DbHandler {
    async fn handle(&self, _event: &OrderPlaced) -> HandlerResult {
        let _ = &self.db;
        Ok(())
    }
}

struct OrderDescriptor;

impl HandlerDescriptor for OrderDescriptor {
    fn register<'a>(
        &'a self,
        bus: &'a EventBus,
        deps: &'a Deps,
    ) -> Pin<Box<dyn Future<Output = Result<Subscription, EventBusError>> + Send + 'a>> {
        Box::pin(async move {
            let db = deps.get_required::<Arc<Db>>()?.clone();
            bus.subscribe::<OrderPlaced, _, _>(DbHandler { db }).await
        })
    }
}
```

When to use it:

- You want full control over registration logic.
- You need custom descriptor behavior not expressed by macro attrs.

## Dependency Paths

There are two main ways to provide dependencies.

### A) Builder DI with `Deps` + `Dep<T>`

Dependencies are inserted into `Deps` and resolved at `build()` time.

```rust,ignore
use std::sync::Arc;
use jaeb::{Dep, Deps, EventBus, HandlerResult, handler};

#[derive(Clone)]
struct Db;

#[derive(Clone)]
struct OrderPlaced;

#[handler]
async fn with_destructure(_event: &OrderPlaced, Dep(db): Dep<Arc<Db>>) -> HandlerResult {
    let _ = db;
    Ok(())
}

#[handler]
fn with_wrapper(_event: &OrderPlaced, db: Dep<Arc<Db>>) -> HandlerResult {
    let _inner: Arc<Db> = db.0;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), jaeb::EventBusError> {
    let db = Arc::new(Db);
    let bus = EventBus::builder()
        .handler(with_destructure)
        .handler(with_wrapper)
        .deps(Deps::new().insert(db))
        .build()
        .await?;
    bus.shutdown().await
}
```

Notes:

- Supported parameter forms:
  - `Dep(name): Dep<T>`
  - `name: Dep<T>`
- Missing dependencies return `EventBusError::MissingDependency` at build time.
- `T` must be `Clone`; for non-`Clone` resources, inject `Arc<T>`.

### B) Self-contained handlers (no `Deps`)

Construct handler structs manually with their own fields and subscribe directly.

```rust,ignore
use std::sync::Arc;
use jaeb::{EventBus, EventHandler, HandlerResult};

#[derive(Clone)]
struct Db;

#[derive(Clone)]
struct OrderPlaced;

struct DbHandler {
    db: Arc<Db>,
}

impl EventHandler<OrderPlaced> for DbHandler {
    async fn handle(&self, _event: &OrderPlaced) -> HandlerResult {
        let _ = &self.db;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), jaeb::EventBusError> {
    let bus = EventBus::builder().build().await?;
    let db = Arc::new(Db);
    let _sub = bus.subscribe::<OrderPlaced, _, _>(DbHandler { db }).await?;
    bus.shutdown().await
}
```

## Dead-letter specifics

- Use `#[dead_letter_handler]` for dead-letter functions.
- Dead-letter handlers are sync-only and receive `&DeadLetter`.
- Register via `EventBusBuilder::dead_letter(...)` or `bus.subscribe_dead_letters(...)`.
- Dead-letter recursion is prevented: failures in dead-letter handlers do not
  emit additional dead letters.

## Middleware (brief)

Middleware registration is independent of the handler descriptor patterns above.
Use:

- `add_middleware` / `add_sync_middleware` for global middleware
- `add_typed_middleware` / `add_typed_sync_middleware` for event-type-scoped middleware

See `examples/middleware`.
