use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

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
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let sub = bus.subscribe(Counter { count: Arc::clone(&count) }).await.expect("subscribe");

    bus.publish(Tick).await.expect("publish");
    assert_eq!(count.load(Ordering::SeqCst), 1);

    sub.unsubscribe().await.expect("unsubscribe");

    bus.publish(Tick).await.expect("publish after unsubscribe");
    assert_eq!(count.load(Ordering::SeqCst), 1); // unchanged

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn unsubscribe_by_id_stops_delivery() {
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let sub = bus.subscribe(Counter { count: Arc::clone(&count) }).await.expect("subscribe");
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
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");

    // Subscribe and immediately unsubscribe to obtain a valid-but-removed id.
    let sub = bus
        .subscribe(Counter {
            count: Arc::new(AtomicUsize::new(0)),
        })
        .await
        .expect("subscribe");
    let id = sub.id();
    sub.unsubscribe().await.expect("first unsubscribe");

    // Now `id` is no longer tracked by the runtime state — should return Ok(false).
    let removed = bus.unsubscribe(id).await.expect("second unsubscribe");
    assert!(!removed);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn double_unsubscribe_returns_false_second_time() {
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let sub = bus.subscribe(Counter { count: Arc::clone(&count) }).await.expect("subscribe");
    let id = sub.id();

    let first = sub.unsubscribe().await.expect("first unsubscribe");
    assert!(first);

    let second = bus.unsubscribe(id).await.expect("second unsubscribe");
    assert!(!second);

    bus.shutdown().await.expect("shutdown");
}

/// Concurrent publish and unsubscribe should not panic or deadlock.
///
/// This test exercises the race between publish (which snapshots the
/// listener list) and unsubscribe (which mutates it). The bus uses
/// copy-on-write (`Arc<Vec<Listener>>`) so both operations are safe.
#[tokio::test]
async fn concurrent_publish_and_unsubscribe_is_safe() {
    let bus = EventBus::builder().buffer_size(256).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    // Register several listeners.
    let mut subs = Vec::new();
    for _ in 0..10 {
        let sub = bus.subscribe(Counter { count: Arc::clone(&count) }).await.expect("subscribe");
        subs.push(sub);
    }

    let mut tasks = tokio::task::JoinSet::new();

    // Spawn concurrent publishers.
    let bus_pub = bus.clone();
    tasks.spawn(async move {
        for _ in 0..100 {
            let _ = bus_pub.publish(Tick).await;
            tokio::task::yield_now().await;
        }
    });

    // Concurrently unsubscribe half the listeners.
    let bus_unsub = bus.clone();
    tasks.spawn(async move {
        for sub in subs.drain(..5) {
            let _ = bus_unsub.unsubscribe(sub.id()).await;
            tokio::task::yield_now().await;
        }
    });

    // All tasks should complete promptly and without panic.
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(result) = tasks.join_next().await {
            result.expect("concurrent task should not panic");
        }
    })
    .await
    .expect("concurrent publish/unsubscribe timed out");

    // Bus remains operational after the race.
    bus.publish(Tick).await.expect("publish after race should succeed");
    assert!(bus.is_healthy().await, "bus should remain healthy after race");

    // The counter should have been incremented some number of times — we
    // don't assert an exact count because the ordering is non-deterministic.
    assert!(count.load(Ordering::SeqCst) > 0, "at least some events should have been delivered");

    bus.shutdown().await.expect("shutdown");
}
