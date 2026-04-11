//! Demonstrates `#[handler]` with auto-discovered `register_handlers!(bus)`.

use jaeb::{EventBus, HandlerResult, handler, register_handlers};

#[derive(Clone, Debug)]
struct UserCreated {
    id: u64,
}

#[handler]
async fn on_user_created(event: &UserCreated) -> HandlerResult {
    println!("user created: {}", event.id);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), jaeb::EventBusError> {
    let bus = EventBus::new(64)?;

    register_handlers!(bus)?;

    bus.publish(UserCreated { id: 1 }).await?;

    bus.shutdown().await
}
