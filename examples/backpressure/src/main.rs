//! Backpressure: non-blocking `try_publish` and `ChannelFull` handling.
//!
//! With a small buffer, the bus applies back-pressure when handlers
//! cannot keep up with the publish rate.

use std::time::Duration;

use jaeb::{EventBus, EventBusError, EventHandler, HandlerResult};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Task {
    id: u32,
}

// ── Handlers ────────────────────────────────────────────────────────────

struct SlowHandler;

impl EventHandler<Task> for SlowHandler {
    async fn handle(&self, event: &Task) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(200)).await;
        println!("processed task {}", event.id);
        Ok(())
    }
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Small buffer of 2 permits.
    let bus = EventBus::builder().buffer_size(2).build().expect("valid config");

    let _ = bus.subscribe::<Task, _, _>(SlowHandler).await.expect("subscribe failed");

    // try_publish is non-blocking — it spawns dispatch and returns immediately.
    bus.try_publish(Task { id: 1 }).expect("try_publish 1 failed");
    bus.try_publish(Task { id: 2 }).expect("try_publish 2 failed");
    println!("two tasks queued");

    // Third try_publish fails because both permits are held by in-flight handlers.
    match bus.try_publish(Task { id: 3 }) {
        Err(EventBusError::ChannelFull) => println!("try_publish 3: ChannelFull (expected)"),
        other => panic!("expected ChannelFull, got {other:?}"),
    }

    // Blocking publish waits until a permit becomes available.
    println!("blocking publish waiting for a free slot...");
    bus.publish(Task { id: 3 }).await.expect("publish failed");
    println!("blocking publish succeeded");

    tokio::time::sleep(Duration::from_millis(500)).await;
    bus.shutdown().await.expect("shutdown failed");
}
