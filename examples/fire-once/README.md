# fire-once

Demonstrates `subscribe_once`: the handler fires exactly once on the first
matching event and then automatically unsubscribes itself.

## Run

```bash
cargo run -p fire-once
```

## What it demonstrates

| Concept | Where |
|---|---|
| `bus.subscribe_once` | Registers a single-fire handler that removes itself after the first invocation |
| Self-removal verified | `bus.stats().total_subscriptions` drops to 0 after the first publish |
| Subsequent publishes ignored | Counter stays at 1 despite three publishes |

## Expected output

```
initialized!
after publish #1: count=1
after publish #2: count=1
after publish #3: count=1
remaining subscriptions: 0
```
