// SPDX-License-Identifier: MIT
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{EventBus, HandlerResult, SyncEventHandler};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Signal;

// ── Handlers ─────────────────────────────────────────────────────────

struct Counter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<Signal> for Counter {
    fn handle(&self, _event: &Signal) -> HandlerResult {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn cloned_bus_shares_state() {
    let bus = EventBus::new(16).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    // Subscribe on the original handle.
    let _ = bus.subscribe(Counter { count: Arc::clone(&count) }).await.expect("subscribe");

    // Clone and publish on the clone.
    let bus2 = bus.clone();
    bus2.publish(Signal).await.expect("publish via clone");

    assert_eq!(count.load(Ordering::SeqCst), 1);

    // Publish on original also works.
    bus.publish(Signal).await.expect("publish via original");
    assert_eq!(count.load(Ordering::SeqCst), 2);

    bus.shutdown().await.expect("shutdown");
}
