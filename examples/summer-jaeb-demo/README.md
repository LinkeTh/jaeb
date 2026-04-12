# summer-jaeb-demo

A web application example showing the `summer-jaeb` plugin integrated into a
[summer-rs](https://github.com/summer-rs/summer) app. Event listeners are
**auto-discovered** via `#[event_listener]` — no manual wiring required.

## Run

```bash
cargo run -p summer-jaeb-demo
```

- HTTP server: `http://localhost:8080`
- OpenAPI docs (Scalar UI): `http://localhost:8080/docs/scalar`

## Try it

```bash
# Place an order (publishes OrderPlacedEvent → async listener with retry + dead-letter)
curl -X POST http://localhost:8080/orders \
  -H "Content-Type: application/json" \
  -d '{"order_id": 42}'

# Ship an order (publishes OrderShippedEvent → sync listener)
curl -X POST http://localhost:8080/orders/42/ship
```

## What it demonstrates

| Concept                               | Where                                                                                       |
|---------------------------------------|---------------------------------------------------------------------------------------------|
| `#[event_listener]` macro             | All three listeners — no struct boilerplate needed                                          |
| Async listener + failure policy attrs | `on_order_placed` with `retries`, `retry_strategy`, `dead_letter`                           |
| Sync listener                         | `on_order_shipped` — executes inline during `publish`                                       |
| Dead-letter listener (auto-detected)  | `on_dead_letter` — macro infers `subscribe_dead_letters()` from the `DeadLetter` event type |
| Component injection (`Component<T>`)  | `on_order_placed` receives a `DbPool` resolved from summer's DI registry                    |
| Plugin dependency ordering            | `.with_dependency("DbPoolPlugin")` ensures `DbPool` is registered before listener wiring    |
| TOML configuration                    | `config/app.toml` sets `buffer_size`, `handler_timeout_secs`, and web port                  |
| HTTP endpoints publishing events      | `place_order` and `ship_order` inject `EventBus` via `WebComponent<EventBus>`               |
| OpenAPI schema generation             | `JsonSchema` derive on request/response types, docs at `/docs`                              |

## Plugin startup order

Startup order is non-deterministic, to ensure the `DbPool` is available we have to specify the dependency:

```rust,ignore
.add_plugin(SummerJaeb::new().with_dependency("DbPoolPlugin"))
```
