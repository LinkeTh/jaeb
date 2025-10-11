# JAEB - Just Another Event Bus

Spring Boot Application Event like implementation.

---

## Work in progress.

---

Setup event bus and bootstrap listeners

```rust
#[tokio::main]
async fn main() {
    let bus = EventBus::new(64);
    // Automatically registers all #[event_listener] functions in the binary
    bootstrap_listeners!(&bus);
}
```

Mark event listener functions.

```rust
#[event_listener]
async fn on_checkout_async(e: OrderCheckOutEvent) {
    // do async work, e.g. send email, call another service
    info!("async: order {} checked out", e.order_id);
}

#[event_listener]
fn on_checkout(e: &OrderCheckOutEvent) {
    // do synchronous side effects
    info!("sync: order {} checked out", e.order_id);
}
```

Publish events

```rust
async fn checkout(order_id: i32, bus: &EventBus) {
    // domain logic ...
    bus.publish(OrderCheckOutEvent { order_id }).await;
}
```

_This waits for synchronous listeners to complete, but may return before asynchronously-registered listeners finish their work._