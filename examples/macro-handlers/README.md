# macro-handlers

Demonstrates `#[handler]` and `#[dead_letter_handler]` with builder-based
registration and `Dep<T>` dependency injection.

## Run

```bash
cargo run -p macro-handlers
```

## What it demonstrates

| Concept | Where |
|---|---|
| `#[handler]` and `#[dead_letter_handler]` | `process_payment` and `on_dead_letter` |
| `Dep<T>` injection via builder | `.deps(Deps::new().insert(...))` passed to both handlers |
| Builder descriptor registration | `.handler(process_payment)` and `.dead_letter(on_dead_letter)` |
| Full failure pipeline | `process_payment` always fails → retries → dead letter → `on_dead_letter` fires |

## Expected output

```
dead-letter: event=Payment, handler=Some("payment-processor"), attempts=3, error=simulated failure for payment 7
```
