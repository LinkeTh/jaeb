//! Panic safety: a handler that panics does not crash the bus.
//! Other handlers continue to operate normally.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{EventBus, HandlerResult, SyncEventHandler};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Boom;

#[derive(Clone, Debug)]
struct Safe;

// ── Handlers ────────────────────────────────────────────────────────────

struct PanicHandler;

impl SyncEventHandler<Boom> for PanicHandler {
    fn handle(&self, _event: &Boom) -> HandlerResult {
        panic!("handler exploded!");
    }
}

struct SafeCounter(Arc<AtomicUsize>);

impl SyncEventHandler<Safe> for SafeCounter {
    fn handle(&self, _event: &Safe) -> HandlerResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        println!("safe handler invoked");
        Ok(())
    }
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe::<Boom, _, _>(PanicHandler).await.expect("subscribe failed");
    let _ = bus
        .subscribe::<Safe, _, _>(SafeCounter(Arc::clone(&count)))
        .await
        .expect("subscribe failed");

    // This handler panics internally — the bus catches it.
    bus.publish(Boom).await.expect("publish failed");

    // The bus is still alive; other handlers work fine.
    bus.publish(Safe).await.expect("publish failed");

    assert_eq!(count.load(Ordering::SeqCst), 1);
    println!("bus survived the panic, safe count = 1");

    bus.shutdown().await.expect("shutdown failed");
}
