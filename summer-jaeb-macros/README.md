# summer-jaeb-macros

[![crates.io](https://img.shields.io/crates/v/summer-jaeb-macros.svg)](https://crates.io/crates/summer-jaeb-macros)
[![docs.rs](https://docs.rs/summer-jaeb-macros/badge.svg)](https://docs.rs/summer-jaeb-macros)
[![license](https://img.shields.io/crates/l/summer-jaeb-macros.svg)](https://github.com/LinkeTh/jaeb/blob/main/LICENSE)

Proc-macro crate for
[`summer-jaeb`](https://crates.io/crates/summer-jaeb).

It provides the `#[event_listener]` attribute used to declare event listeners
that are auto-registered by the `SummerJaeb` plugin via `inventory`.

## What the macro generates

For each annotated function, the macro generates:

- a handler type implementing `jaeb::EventHandler<E>` (async) or
  `jaeb::SyncEventHandler<E>` (sync)
- a registrar implementing `summer_jaeb::TypedListenerRegistrar`
- an `inventory::submit!` entry discovered at plugin startup

## Supported forms

```rust,ignore
#[event_listener]
async fn on_event(event: &MyEvent) -> HandlerResult {
    Ok(())
}

// Custom listener name (appears in traces, dead letters, and stats)
#[event_listener(name = "order-processor")]
async fn on_event_named(event: &MyEvent) -> HandlerResult {
    Ok(())
}

// Opt out of automatic naming (defaults to function name when omitted)
#[event_listener(name = "")]
async fn on_event_unnamed(event: &MyEvent) -> HandlerResult {
    Ok(())
}

// Fixed delay
#[event_listener(retries = 3, retry_strategy = "fixed", retry_base_ms = 100, dead_letter = true)]
async fn on_event_with_retries(event: &MyEvent) -> HandlerResult {
    Ok(())
}

// Explicit retry strategy — exponential back-off
#[event_listener(retries = 5, retry_strategy = "exponential", retry_base_ms = 50, retry_max_ms = 5000)]
async fn on_event_exp(event: &MyEvent) -> HandlerResult {
    Ok(())
}

// Exponential back-off with jitter
#[event_listener(retries = 5, retry_strategy = "exponential_jitter", retry_base_ms = 50, retry_max_ms = 5000)]
async fn on_event_jitter(event: &MyEvent) -> HandlerResult {
    Ok(())
}

// Explicit fixed strategy
#[event_listener(retries = 3, retry_strategy = "fixed", retry_base_ms = 100)]
async fn on_event_fixed(event: &MyEvent) -> HandlerResult {
    Ok(())
}
```

State extraction parameters must use:

`Component(name): Component<Type>`

## Constraints

- target must be a free function (not a method)
- first parameter must be `&EventType`
- return type must be explicit (`-> HandlerResult`)
- `DeadLetter` listeners must be synchronous
- failure-policy attributes are rejected for `DeadLetter` listeners
- `retry_base_ms` / `retry_max_ms` require `retry_strategy`
- `retry_strategy = "exponential"` and `"exponential_jitter"` require both `retry_base_ms` and `retry_max_ms`
- `retry_strategy = "fixed"` requires `retry_base_ms`
- all retry attributes require `retries = N`
- retry attributes are only supported on async handlers

## Intended usage

You typically import `event_listener` from `summer-jaeb`:

```rust,ignore
use summer_jaeb::event_listener;
```

Direct dependency on this crate is usually not necessary.

## License

MIT
