use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use jaeb::{EventBus, EventHandler, HandlerResult, SyncEventHandler};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct EventA;

#[derive(Clone, Debug)]
struct EventB;

// ── Handlers ─────────────────────────────────────────────────────────

struct SyncHandlerA;
impl SyncEventHandler<EventA> for SyncHandlerA {
    fn handle(&self, _event: &EventA, _bus: &EventBus) -> HandlerResult {
        Ok(())
    }
    fn name(&self) -> Option<&'static str> {
        Some("handler-a")
    }
}

struct SyncHandlerB;
impl SyncEventHandler<EventB> for SyncHandlerB {
    fn handle(&self, _event: &EventB, _bus: &EventBus) -> HandlerResult {
        Ok(())
    }
}

struct SlowAsyncHandler {
    started: Arc<AtomicBool>,
}
impl EventHandler<EventA> for SlowAsyncHandler {
    async fn handle(&self, _event: &EventA, _bus: &EventBus) -> HandlerResult {
        self.started.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    }
    fn name(&self) -> Option<&'static str> {
        Some("slow-async")
    }
}

struct BlockingAsyncHandler {
    starts: Arc<AtomicUsize>,
    release: Arc<tokio::sync::Notify>,
}

impl EventHandler<EventA> for BlockingAsyncHandler {
    async fn handle(&self, _event: &EventA, _bus: &EventBus) -> HandlerResult {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.release.notified().await;
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn stats_empty_bus() {
    let bus = EventBus::builder().build().await.expect("valid config");

    let stats = bus.stats().await.expect("stats");
    assert_eq!(stats.total_subscriptions, 0);
    assert!(stats.registered_event_types.is_empty());
    assert!(stats.subscriptions_by_event.is_empty());
    assert_eq!(stats.dispatches_in_flight, 0);
    assert_eq!(stats.in_flight_async, 0);
    assert!(!stats.shutdown_called);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn stats_after_subscriptions() {
    let bus = EventBus::builder().build().await.expect("valid config");

    let _sub_a = bus.subscribe(SyncHandlerA).await.expect("subscribe a");
    let _sub_b = bus.subscribe(SyncHandlerB).await.expect("subscribe b");

    let stats = bus.stats().await.expect("stats");
    assert_eq!(stats.total_subscriptions, 2);
    assert_eq!(stats.registered_event_types.len(), 2);

    // EventA should have 1 listener named "handler-a"
    let event_a_name = stats
        .registered_event_types
        .iter()
        .find(|n| n.contains("EventA"))
        .expect("EventA should be registered");
    let a_listeners = &stats.subscriptions_by_event[event_a_name];
    assert_eq!(a_listeners.len(), 1);
    assert_eq!(a_listeners[0].name, Some("handler-a"));

    // EventB should have 1 unnamed listener
    let event_b_name = stats
        .registered_event_types
        .iter()
        .find(|n| n.contains("EventB"))
        .expect("EventB should be registered");
    let b_listeners = &stats.subscriptions_by_event[event_b_name];
    assert_eq!(b_listeners.len(), 1);
    assert_eq!(b_listeners[0].name, None);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn stats_in_flight_async() {
    let bus = EventBus::builder()
        .shutdown_timeout(Duration::from_millis(100))
        .build()
        .await
        .expect("valid config");
    let started = Arc::new(AtomicBool::new(false));

    let _sub = bus
        .subscribe(SlowAsyncHandler {
            started: Arc::clone(&started),
        })
        .await
        .expect("subscribe");

    bus.publish(EventA).await.expect("publish");

    // Wait for the async handler to start running.
    tokio::time::timeout(Duration::from_secs(2), async {
        while !started.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("handler should have started");

    let stats = bus.stats().await.expect("stats");
    assert!(
        stats.in_flight_async >= 1,
        "expected at least 1 in-flight async task, got {}",
        stats.in_flight_async
    );

    // Shutdown with timeout — the slow handler will be aborted.
    let _ = bus.shutdown().await;
}

#[tokio::test]
async fn stats_shutdown_called() {
    let bus = EventBus::builder().build().await.expect("valid config");

    let stats = bus.stats().await.expect("stats before shutdown");
    assert!(!stats.shutdown_called);

    bus.shutdown().await.expect("shutdown");

    // After shutdown, stats() should return Stopped.
    let result = bus.stats().await;
    assert!(result.is_err(), "stats after shutdown should fail");
}

#[tokio::test]
async fn stats_in_flight_async_counts_per_listener_tasks() {
    let bus = EventBus::builder()
        .shutdown_timeout(Duration::from_millis(100))
        .build()
        .await
        .expect("valid config");

    let starts = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());

    for _ in 0..3 {
        let _sub = bus
            .subscribe(BlockingAsyncHandler {
                starts: Arc::clone(&starts),
                release: Arc::clone(&release),
            })
            .await
            .expect("subscribe");
    }

    bus.publish(EventA).await.expect("publish");

    tokio::time::timeout(Duration::from_secs(2), async {
        while starts.load(Ordering::SeqCst) < 3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("all async listeners should start");

    let stats = bus.stats().await.expect("stats");
    assert!(
        stats.in_flight_async >= 3,
        "expected at least 3 in-flight async tasks for 3 listeners, got {}",
        stats.in_flight_async
    );

    for _ in 0..3 {
        release.notify_one();
    }

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn stats_after_unsubscribe() {
    let bus = EventBus::builder().build().await.expect("valid config");

    let sub = bus.subscribe(SyncHandlerA).await.expect("subscribe");

    let stats = bus.stats().await.expect("stats");
    assert_eq!(stats.total_subscriptions, 1);

    sub.unsubscribe().await.expect("unsubscribe");

    let stats = bus.stats().await.expect("stats");
    assert_eq!(stats.total_subscriptions, 0);
    // Event type should still be tracked but with empty listener list removed.
    // After removing the last listener for a type, the entry is cleaned up.
    assert!(stats.registered_event_types.is_empty());

    bus.shutdown().await.expect("shutdown");
}
