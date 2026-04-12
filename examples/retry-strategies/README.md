# retry-strategies

Compares all three built-in retry delay strategies on handlers that fail a
configurable number of times before succeeding.

## Run

```bash
cargo run -p retry-strategies
```

## Strategies

| Strategy | Configuration | Delay pattern |
|---|---|---|
| `Fixed` | `RetryStrategy::Fixed(Duration)` | Constant delay between every retry |
| `Exponential` | `RetryStrategy::Exponential { base, max }` | Doubles each retry, capped at `max` |
| `ExponentialWithJitter` | `RetryStrategy::ExponentialWithJitter { base, max }` | Randomised delay up to the exponential cap |

## What it demonstrates

| Concept | Where |
|---|---|
| `SubscriptionPolicy::with_retry_strategy` | Sets the delay strategy for a subscription |
| `with_max_retries(n)` | Maximum number of retry attempts after the initial failure |
| `with_dead_letter(false)` | Disables dead-letter routing so terminal failures are silently dropped |
| Attempt counting | `FlakyHandler` uses `Arc<AtomicUsize>` to fail the first N attempts |
