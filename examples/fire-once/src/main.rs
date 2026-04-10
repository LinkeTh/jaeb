//! `subscribe_once`: the handler fires exactly once and auto-removes itself.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use jaeb::{EventBus, EventHandler, HandlerResult};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Initialized;

// ── Handlers ────────────────────────────────────────────────────────────

struct OnInit(Arc<AtomicUsize>);

impl EventHandler<Initialized> for OnInit {
    async fn handle(&self, _event: &Initialized) -> HandlerResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        println!("initialized!");
        Ok(())
    }
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _ = bus
        .subscribe_once::<Initialized, _, _>(OnInit(Arc::clone(&count)))
        .await
        .expect("subscribe_once failed");

    // Publish three times — handler fires only on the first.
    for i in 1..=3 {
        bus.publish(Initialized).await.expect("publish failed");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        println!("after publish #{i}: count={}", count.load(Ordering::SeqCst));
    }

    // The listener was automatically removed after the first fire.
    let stats = bus.stats().await.expect("stats failed");
    println!("remaining subscriptions: {}", stats.total_subscriptions);

    bus.shutdown().await.expect("shutdown failed");
}
