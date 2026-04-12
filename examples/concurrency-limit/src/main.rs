//! Concurrency limit: `max_concurrent_async` caps parallel handler execution.
//!
//! Without this cap every published event spawns an async task immediately.
//! With the cap, excess tasks queue behind a semaphore.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jaeb::{EventBus, EventHandler, HandlerResult};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Work {
    id: u32,
}

// ── Handlers ────────────────────────────────────────────────────────────

struct TrackedHandler {
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl EventHandler<Work> for TrackedHandler {
    async fn handle(&self, event: &Work) -> HandlerResult {
        let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        // Update peak concurrency.
        self.peak.fetch_max(current, Ordering::SeqCst);

        tokio::time::sleep(Duration::from_millis(100)).await;
        println!("processed work {}", event.id);

        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    let bus = EventBus::builder()
        .buffer_size(64)
        .max_concurrent_async(2)
        .build()
        .await
        .expect("valid config");

    let _ = bus
        .subscribe::<Work, _, _>(TrackedHandler {
            in_flight: Arc::clone(&in_flight),
            peak: Arc::clone(&peak),
        })
        .await
        .expect("subscribe failed");

    // Publish 8 events rapidly.
    for id in 1..=8 {
        bus.publish(Work { id }).await.expect("publish failed");
    }

    // Wait for all handlers to finish.
    tokio::time::sleep(Duration::from_secs(1)).await;
    bus.shutdown().await.expect("shutdown failed");

    let observed_peak = peak.load(Ordering::SeqCst);
    println!("peak concurrency: {observed_peak} (limit was 2)");
    assert!(observed_peak <= 2, "concurrency cap violated");
}
