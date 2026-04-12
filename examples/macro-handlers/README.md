# macro-handlers

Demonstrates the `#[handler]` macro with full policy attributes and
explicit handler listing in `register_handlers!`.

## Run

```bash
cargo run -p macro-handlers
```

## What it demonstrates

| Concept | Where |
|---|---|
| `#[handler]` with policy attrs | `retries`, `retry_strategy`, `retry_base_ms`, `dead_letter`, `name` on `process_payment` |
| Dead-letter handler via macro | `#[handler]` on a sync `fn` with `&DeadLetter` arg; auto-detected and wired via `subscribe_dead_letters` |
| `register_handlers!(bus, fn1, fn2)` | Explicit handler listing — only the named functions are subscribed |
| Full failure pipeline | `process_payment` always fails → 2 retries → dead letter → `log_dead_letter` fires |

## Expected output

```
dead-letter: event=Payment, handler=Some("payment-processor"), attempts=3, error=simulated failure for payment 7
```
