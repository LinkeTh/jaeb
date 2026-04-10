//! Minimal publish/subscribe with an async `EventHandler`.

use jaeb::{EventBus, EventHandler, HandlerResult};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct UserCreated {
    id: u32,
    name: &'static str,
}

// ── Handlers ────────────────────────────────────────────────────────────

struct OnUserCreated;

impl EventHandler<UserCreated> for OnUserCreated {
    async fn handle(&self, event: &UserCreated) -> HandlerResult {
        println!("user created: id={}, name={}", event.id, event.name);
        Ok(())
    }
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let bus = EventBus::new(64).expect("valid config");

    let _ = bus.subscribe::<UserCreated, _, _>(OnUserCreated).await.expect("subscribe failed");

    bus.publish(UserCreated { id: 1, name: "Alice" }).await.expect("publish failed");

    // Give the async handler time to complete.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    bus.shutdown().await.expect("shutdown failed");

    println!("done");
}
