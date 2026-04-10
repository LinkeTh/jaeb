//! Synchronous dispatch with `SyncEventHandler`.
//!
//! Sync handlers run inline during `publish` in a serialized FIFO lane
//! per event type.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use jaeb::{EventBus, HandlerResult, SyncEventHandler};

// ── Events ──────────────────────────────────────────────────────────────

/// `Clone` is required by `publish()` even though sync handlers receive `&E`.
#[derive(Clone, Debug)]
struct OrderPlaced {
    id: u32,
}

// ── Handlers ────────────────────────────────────────────────────────────

struct FirstHandler(Arc<AtomicUsize>);
struct SecondHandler(Arc<AtomicUsize>);

impl SyncEventHandler<OrderPlaced> for FirstHandler {
    fn handle(&self, event: &OrderPlaced) -> HandlerResult {
        let seq = self.0.fetch_add(1, Ordering::SeqCst);
        println!("[seq={seq}] first handler: order {} placed", event.id);
        Ok(())
    }
}

impl SyncEventHandler<OrderPlaced> for SecondHandler {
    fn handle(&self, event: &OrderPlaced) -> HandlerResult {
        let seq = self.0.fetch_add(1, Ordering::SeqCst);
        println!("[seq={seq}] second handler: order {} placed", event.id);
        Ok(())
    }
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let counter = Arc::new(AtomicUsize::new(0));

    let bus = EventBus::new(64).expect("valid config");

    let _ = bus
        .subscribe::<OrderPlaced, _, _>(FirstHandler(Arc::clone(&counter)))
        .await
        .expect("subscribe failed");
    let _ = bus
        .subscribe::<OrderPlaced, _, _>(SecondHandler(Arc::clone(&counter)))
        .await
        .expect("subscribe failed");

    // Both handlers run serially during this call; publish returns after both complete.
    bus.publish(OrderPlaced { id: 42 }).await.expect("publish failed");

    println!("total invocations: {}", counter.load(Ordering::SeqCst));

    bus.shutdown().await.expect("shutdown failed");
}
