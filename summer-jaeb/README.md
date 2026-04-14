# summer-jaeb

[![crates.io](https://img.shields.io/crates/v/summer-jaeb.svg)](https://crates.io/crates/summer-jaeb)
[![docs.rs](https://docs.rs/summer-jaeb/badge.svg)](https://docs.rs/summer-jaeb)
[![license](https://img.shields.io/crates/l/summer-jaeb.svg)](https://github.com/LinkeTh/jaeb/blob/main/LICENSE)

`summer-jaeb` integrates [`jaeb`](https://crates.io/crates/jaeb) with
[summer-rs](https://crates.io/crates/summer) via a plugin and an
attribute macro.

It provides:

- `SummerJaeb` plugin that registers a shared `jaeb::EventBus` component
- auto-registration of listeners discovered via `inventory`
- `#[event_listener]` for concise listener declaration (re-exported from
  `summer-jaeb-macros`)
- optional `metrics` feature passthrough to `jaeb/metrics`

## Installation

```toml
[dependencies]
summer-jaeb = "0.3"
```

Optional metrics support:

```toml
[dependencies]
summer-jaeb = { version = "0.3", features = ["metrics"] }
```

## Usage

Register the plugin in your Summer app:

```rust,ignore
use summer::app::App;
use summer_jaeb::SummerJaeb;

#[tokio::main]
async fn main() {
    App::new().add_plugin(SummerJaeb).run().await;
}
```

Declare listeners with `#[event_listener]`:

```rust,ignore
use jaeb::HandlerResult;
use summer_jaeb::event_listener;

#[derive(Clone)]
struct OrderPlaced {
    order_id: u64,
}

#[event_listener]
async fn on_order_placed(event: &OrderPlaced) -> HandlerResult {
    tracing::info!(order_id = event.order_id, "order placed");
    Ok(())
}
```

Configuration (`app.toml`):

```toml
[jaeb]
handler_timeout_secs = 5
max_concurrent_async = 100
shutdown_timeout_secs = 10
```

## Notes

- `DeadLetter` listeners must be synchronous.
- `#[event_listener]` requires an explicit return type (`-> HandlerResult`).
- Additional parameters use Summer extraction syntax:
  `Component(name): Component<Type>`.

See the demo at
[`examples/summer-jaeb-demo`](../examples/summer-jaeb-demo).

## License

MIT
