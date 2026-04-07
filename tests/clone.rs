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
    let bus = EventBus::new(16);
    let count = Arc::new(AtomicUsize::new(0));

    // Register on the original handle.
    bus.register(Counter { count: Arc::clone(&count) }).await.expect("register");

    // Clone and publish on the clone.
    let bus2 = bus.clone();
    bus2.publish(Signal).await.expect("publish via clone");

    assert_eq!(count.load(Ordering::SeqCst), 1);

    // Publish on original also works.
    bus.publish(Signal).await.expect("publish via original");
    assert_eq!(count.load(Ordering::SeqCst), 2);

    bus.shutdown().await.expect("shutdown");
}
