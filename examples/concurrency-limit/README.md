# concurrency-limit

Shows how `max_concurrent_async` caps the number of async handler tasks running
in parallel across the bus.

## Run

```bash
cargo run -p concurrency-limit
```

## What it demonstrates

| Concept                                       | Where                                                                  |
|-----------------------------------------------|------------------------------------------------------------------------|
| `EventBus::builder().max_concurrent_async(n)` | Limits in-flight async tasks to `n` at a time                          |
| Semaphore queuing                             | Tasks beyond the cap wait behind a semaphore rather than being dropped |
| Peak concurrency tracking                     | `TrackedHandler` uses `Arc<AtomicUsize>` to measure actual peak        |
| Assertion                                     | `assert!(peak <= 2)` verifies the cap is never violated                |

With 8 events published and a limit of 2, each batch of 2 handlers runs for
100 ms before the next batch starts.
