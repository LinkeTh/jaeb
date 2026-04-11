# jaeb-macros

Proc macros for [`jaeb`](https://crates.io/crates/jaeb):

- `#[handler]` - turns a free function into a generated JAEB handler struct.
- `register_handlers!(bus, ...)` - batch-registers generated handlers (explicit mode).
- `register_handlers!(bus)` - auto-discovers and registers all `#[handler]` functions.

This crate is also re-exported from `jaeb` behind the `macros` feature.

## Example

```rust,ignore
use jaeb::{EventBus, HandlerResult, handler, register_handlers};

#[derive(Clone)]
struct OrderPlaced {
    id: u64,
}

#[handler]
async fn on_order(event: &OrderPlaced) -> HandlerResult {
    println!("order {}", event.id);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), jaeb::EventBusError> {
    let bus = EventBus::new(64)?;
    register_handlers!(bus, on_order)?;
    // register_handlers!(bus)?;
    bus.publish(OrderPlaced { id: 42 }).await?;
    bus.shutdown().await
}
```
