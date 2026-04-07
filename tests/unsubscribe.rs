use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{EventBus, HandlerResult, SyncEventHandler};

// ── Event / handler types ────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Tick;

struct Counter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<Tick> for Counter {
    fn handle(&self, _event: &Tick) -> HandlerResult {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn unsubscribe_stops_delivery() {
    let bus = EventBus::new(16);
    let count = Arc::new(AtomicUsize::new(0));

    let sub = bus.register(Counter { count: Arc::clone(&count) }).await.expect("register");

    bus.publish(Tick).await.expect("publish");
    assert_eq!(count.load(Ordering::SeqCst), 1);

    sub.unsubscribe().await.expect("unsubscribe");

    bus.publish(Tick).await.expect("publish after unsubscribe");
    assert_eq!(count.load(Ordering::SeqCst), 1); // unchanged

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn unsubscribe_by_id_stops_delivery() {
    let bus = EventBus::new(16);
    let count = Arc::new(AtomicUsize::new(0));

    let sub = bus.register(Counter { count: Arc::clone(&count) }).await.expect("register");
    let id = sub.id();

    bus.publish(Tick).await.expect("publish");
    assert_eq!(count.load(Ordering::SeqCst), 1);

    // Use the bus-level unsubscribe with the raw id.
    let removed = bus.unsubscribe(id).await.expect("unsubscribe by id");
    assert!(removed);

    bus.publish(Tick).await.expect("publish after unsubscribe");
    assert_eq!(count.load(Ordering::SeqCst), 1);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn unsubscribe_unknown_id_returns_false() {
    let bus = EventBus::new(16);

    // Register and immediately unsubscribe to obtain a valid-but-removed id.
    let sub = bus
        .register(Counter {
            count: Arc::new(AtomicUsize::new(0)),
        })
        .await
        .expect("register");
    let id = sub.id();
    sub.unsubscribe().await.expect("first unsubscribe");

    // Now `id` is no longer tracked by the actor — should return Ok(false).
    let removed = bus.unsubscribe(id).await.expect("second unsubscribe");
    assert!(!removed);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn double_unsubscribe_returns_false_second_time() {
    let bus = EventBus::new(16);
    let count = Arc::new(AtomicUsize::new(0));

    let sub = bus.register(Counter { count: Arc::clone(&count) }).await.expect("register");
    let id = sub.id();

    let first = sub.unsubscribe().await.expect("first unsubscribe");
    assert!(first);

    let second = bus.unsubscribe(id).await.expect("second unsubscribe");
    assert!(!second);

    bus.shutdown().await.expect("shutdown");
}
