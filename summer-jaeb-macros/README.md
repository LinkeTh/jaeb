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

#[event_listener(retries = 3, retry_delay_ms = 100, dead_letter = true)]
fn on_event_sync(event: &MyEvent) -> HandlerResult {
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

## Intended usage

You typically import `event_listener` from `summer-jaeb`:

```rust,ignore
use summer_jaeb::event_listener;
```

Direct dependency on this crate is usually not necessary.

## License

MIT
