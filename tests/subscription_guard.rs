use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{EventBus, HandlerResult, SyncEventHandler};

// ── Event / handler types ────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Ping;

struct Counter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<Ping> for Counter {
    fn handle(&self, _event: &Ping) -> HandlerResult {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

/// Dropping a `SubscriptionGuard` unsubscribes the listener.
#[tokio::test]
async fn guard_drop_unsubscribes() {
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    {
        let _guard = bus
            .subscribe(Counter { count: Arc::clone(&count) })
            .await
            .expect("subscribe")
            .into_guard();

        bus.publish(Ping).await.expect("publish while guarded");
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    // Guard dropped — listener should be unsubscribed.

    // Give the runtime a chance to process the fire-and-forget unsubscribe.
    tokio::task::yield_now().await;

    bus.publish(Ping).await.expect("publish after guard drop");
    assert_eq!(count.load(Ordering::SeqCst), 1, "listener should not receive events after guard drop");

    bus.shutdown().await.expect("shutdown");
}

/// `disarm()` prevents the guard from unsubscribing on drop.
#[tokio::test]
async fn disarmed_guard_does_not_unsubscribe() {
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    {
        let mut guard = bus
            .subscribe(Counter { count: Arc::clone(&count) })
            .await
            .expect("subscribe")
            .into_guard();

        guard.disarm();
    }
    // Guard dropped but was disarmed — listener should still be active.

    bus.publish(Ping).await.expect("publish after disarmed guard drop");
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "listener should still receive events after disarmed guard drop"
    );

    bus.shutdown().await.expect("shutdown");
}

/// Dropping the guard after shutdown does not panic or hang.
#[tokio::test]
async fn guard_drop_after_shutdown_is_safe() {
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");

    let guard = bus
        .subscribe(Counter {
            count: Arc::new(AtomicUsize::new(0)),
        })
        .await
        .expect("subscribe")
        .into_guard();

    bus.shutdown().await.expect("shutdown");

    // The guard drop sends a fire-and-forget unsubscribe, which should be
    // silently discarded since the bus is stopped.
    drop(guard);
}

/// `id()` returns `Some` before disarm and `None` after.
#[tokio::test]
async fn guard_id_reflects_state() {
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");

    let sub = bus
        .subscribe(Counter {
            count: Arc::new(AtomicUsize::new(0)),
        })
        .await
        .expect("subscribe");

    let expected_id = sub.id();
    let mut guard = sub.into_guard();

    assert_eq!(guard.id(), Some(expected_id), "id() should return the subscription ID");

    guard.disarm();
    assert_eq!(guard.id(), None, "id() should return None after disarm");

    bus.shutdown().await.expect("shutdown");
}
