# axum-integration

Shows how to embed `EventBus` inside an [axum](https://github.com/tokio-rs/axum) web server,
with struct-based dependency injection into handlers, handler priority ordering, a runtime
stats endpoint, and Ctrl-C graceful shutdown.

## Run

```bash
cargo run -p axum-integration
# Server listens on http://127.0.0.1:3000
```

## Endpoints

| Method | Path      | Description                                              |
|--------|-----------|----------------------------------------------------------|
| `GET`  | `/health` | Returns `{"healthy": true/false}` via `bus.is_healthy()` |
| `POST` | `/orders` | Publishes `OrderCreated`, returns `202 Accepted`         |
| `GET`  | `/stats`  | Returns a `BusStats` JSON snapshot                       |

```bash
curl -X POST http://127.0.0.1:3000/orders \
  -H "Content-Type: application/json" \
  -d '{"order_id":"ord-1","customer_email":"alice@example.com","total_cents":4999}'
```

## What it demonstrates

| Concept                           | Where                                                                                                                                                                              |
|-----------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `EventBus` as axum `State`        | `AppState { bus, db, mailer }` cloned into every route handler                                                                                                                     |
| `EventBus::builder()`             | `max_concurrent_async`, `default_subscription_policies`, `handler_timeout`, `shutdown_timeout`                                                                                     |
| Struct-based dependency injection | `NotificationHandler { mailer }`, `InventoryProjectionHandler { db }`, `AuditLogHandler { db }` — deps injected at startup, stored as `Arc` fields, zero overhead at dispatch time |
| Handler priority ordering         | `AuditLogHandler` at priority 50 runs before other sync listeners                                                                                                                   |
| Mixed async + sync handlers       | `NotificationHandler` / `InventoryProjectionHandler` async; `AuditLogHandler` sync                                                                                                 |
| `SyncSubscriptionPolicy`          | Separate policy builder for sync handlers (`with_dead_letter(false)`)                                                                                                              |
| Dead-letter sink                  | `DeadLetterLogger` logs terminal failures                                                                                                                                          |
| `bus.is_healthy()`                | Checked in the `/health` endpoint                                                                                                                                                  |
| `bus.stats()`                     | Exposed as a JSON endpoint at `/stats`                                                                                                                                             |
| Ctrl-C graceful shutdown          | `server.with_graceful_shutdown(...)` calls `bus.shutdown().await`                                                                                                                  |

## Dependency injection pattern

Application services (`DbPool`, `Mailer`) are constructed once in `main()`, wrapped in
`Arc`, and passed into `register_handlers`. Each handler struct stores only the deps it
actually needs:

```rust
struct NotificationHandler {
    mailer: Arc<Mailer>,
}

struct AuditLogHandler {
    db: Arc<DbPool>,
}
```

At registration time the handler is constructed with its deps:

```rust
bus.subscribe_with_policy::<OrderCreated, _, _>(
    NotificationHandler { mailer: mailer.clone() },
    AsyncSubscriptionPolicy::default().with_max_retries(2),
).await?;
```

The bus holds the constructed handler object for its lifetime. At dispatch time it simply
calls `handler.handle(event, &bus)` — no lookup, no allocation, no lock.
