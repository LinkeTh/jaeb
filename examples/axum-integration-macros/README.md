# axum-integration-macros

Shows an [axum](https://github.com/tokio-rs/axum) REST app wired to `jaeb`
using standalone handler macros (`#[handler]` and `#[dead_letter_handler]`) and
builder-time dependency injection via `Dep<T>`.

This is the macro + `Dep<T>` counterpart to `examples/axum-integration`.

## Run

```bash
cargo run -p axum-integration-macros
# Server listens on http://127.0.0.1:3001
```

## Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | Returns `{"healthy": true/false}` via `bus.is_healthy()` |
| `POST` | `/orders` | Publishes `OrderCreated`, returns `202 Accepted` |
| `GET` | `/stats` | Returns a `BusStats` JSON snapshot |

## What it demonstrates

| Concept | Where |
|---|---|
| Macro-based registration | `.handler(notify_customer)`, `.handler(project_inventory)`, `.handler(append_audit)`, `.dead_letter(log_dead_letter)` |
| `Dep<T>` dependency injection | Handler signatures use both `Dep(mailer): Dep<Arc<Mailer>>` and `db: Dep<Arc<DbPool>>` |
| `Deps` container wiring | `.deps(Deps::new().insert(db.clone()).insert(mailer.clone()))` |
| Mixed async + sync handlers | `notify_customer` / `project_inventory` async; `append_audit` sync |
| Per-handler policy attrs | `retries`, `dead_letter`, `priority`, and `name` set in `#[handler(...)]` |
| Dead-letter handling | `#[dead_letter_handler] fn log_dead_letter(...)` |
| Axum + EventBus state | `AppState { bus, db, mailer }` used by routes |

## Notes

- This example enables `jaeb`'s `macros` feature in its local `Cargo.toml`.
- Dependencies are resolved once during `build()`, not on the publish hot path.
