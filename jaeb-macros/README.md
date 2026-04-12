# jaeb-macros

Proc macros for [`jaeb`](https://crates.io/crates/jaeb):

- `#[handler]` - turns a free function into a generated JAEB handler struct.
- `#[dead_letter_handler]` - turns a sync free function on `&DeadLetter` into a
  generated dead-letter descriptor.

This crate is also re-exported from `jaeb` behind the `macros` feature.

## Example

```rust,ignore
use jaeb::{Dep, Deps, EventBus, HandlerResult, handler};
use std::sync::Arc;

#[derive(Clone)]
struct OrderPlaced {
    id: u64,
}

#[derive(Clone)]
struct AuditLog;

#[handler]
async fn on_order(event: &OrderPlaced, Dep(_audit): Dep<Arc<AuditLog>>) -> HandlerResult {
    println!("order {}", event.id);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), jaeb::EventBusError> {
    let bus = EventBus::builder()
        .buffer_size(64)
        .handler(on_order)
        .deps(Deps::new().insert(Arc::new(AuditLog)))
        .build()
        .await?;
    bus.publish(OrderPlaced { id: 42 }).await?;
    bus.shutdown().await
}
```
