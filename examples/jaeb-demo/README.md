# jaeb-demo

A standalone CLI example demonstrating `jaeb`'s core features: async/sync handlers, retry
policies, dead-letter routing, and live Prometheus metrics.

## Run

```bash
cargo run -p jaeb-demo
```

Metrics are exposed at `http://localhost:3000/metrics`. Override log verbosity with
`RUST_LOG=debug cargo run -p jaeb-demo`.

## What it demonstrates

| Concept                                      | Where                                                                                         |
|----------------------------------------------|-----------------------------------------------------------------------------------------------|
| Async handler with transient failure + retry | `OnOrderCheckout` — fails on attempt 1, succeeds on retry                                     |
| Sync handler (serialized FIFO lane)          | `OnOrderCancelled`                                                                            |
| Multiple handlers for the same event         | `OnOrderCheckout` + `CheckoutLogger` both handle `OrderCheckOutEvent`                         |
| Dead-letter pipeline                         | `OnPaymentFailed` always fails → after 2 retries, `DeadLetterLogger` receives the dead letter |
| Custom handler names                         | `fn name()` on each handler struct for tracing/metrics labels                                 |
| `AsyncSubscriptionPolicy` builder            | `with_max_retries`, `with_retry_strategy`, `with_dead_letter`                                 |
| Graceful shutdown                            | `bus.shutdown().await` drains in-flight async tasks                                           |
| Prometheus metrics                           | Counter, histogram, and dead-letter metrics via `metrics-exporter-prometheus`                 |
| Structured tracing                           | `tracing-subscriber` with env-filter and per-thread IDs                                       |

## Event flow

```
OrderCheckOutEvent  →  OnOrderCheckout (async, retry=1)
                    →  CheckoutLogger  (async)
OrderCancelledEvent →  OnOrderCancelled (sync)
PaymentFailedEvent  →  OnPaymentFailed (async, retry=2, dead_letter=true)
                           ↳ DeadLetterLogger (sync, after retries exhausted)
```
