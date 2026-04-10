//! Dynamic subscription management: explicit unsubscribe and RAII guard.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{EventBus, HandlerResult, SyncEventHandler};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Tick;

// ── Handlers ────────────────────────────────────────────────────────────

struct Counter(Arc<AtomicUsize>);

impl SyncEventHandler<Tick> for Counter {
    fn handle(&self, _event: &Tick) -> HandlerResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    // 1. Subscribe and unsubscribe explicitly.
    let sub = bus.subscribe::<Tick, _, _>(Counter(Arc::clone(&count))).await.expect("subscribe failed");

    bus.publish(Tick).await.expect("publish failed");
    println!("after first publish: {}", count.load(Ordering::SeqCst)); // 1

    sub.unsubscribe().await.expect("unsubscribe failed");
    bus.publish(Tick).await.expect("publish failed");
    println!("after unsubscribe + publish: {}", count.load(Ordering::SeqCst)); // still 1

    // 2. RAII guard: auto-unsubscribes when dropped.
    {
        let _guard = bus
            .subscribe::<Tick, _, _>(Counter(Arc::clone(&count)))
            .await
            .expect("subscribe failed")
            .into_guard();

        bus.publish(Tick).await.expect("publish failed");
        println!("inside guard scope: {}", count.load(Ordering::SeqCst)); // 2
        // _guard drops here → listener removed
    }

    // Small yield to let the guard's async unsubscribe complete.
    tokio::task::yield_now().await;

    bus.publish(Tick).await.expect("publish failed");
    println!("after guard dropped + publish: {}", count.load(Ordering::SeqCst)); // still 2

    bus.shutdown().await.expect("shutdown failed");
}
