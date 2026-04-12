//! Tests for per-event-type snapshot-slot architecture.
//!
//! These tests validate structural properties of per-type snapshot slots
//! and dispatch lanes rather than general event bus semantics.

use jaeb::{EventBus, EventHandler, HandlerResult, SyncEventHandler};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Alpha(usize);

#[derive(Clone, Debug)]
struct Beta(usize);

#[derive(Clone, Debug)]
struct Gamma;

// ── Helpers ──────────────────────────────────────────────────────────

struct SyncAlphaRecorder {
    log: Arc<std::sync::Mutex<Vec<usize>>>,
}

impl SyncEventHandler<Alpha> for SyncAlphaRecorder {
    fn handle(&self, event: &Alpha) -> HandlerResult {
        self.log.lock().unwrap().push(event.0);
        Ok(())
    }
}

struct SlowAlphaHandler {
    started_tx: mpsc::UnboundedSender<&'static str>,
    release: Arc<Notify>,
}

impl EventHandler<Alpha> for SlowAlphaHandler {
    async fn handle(&self, _event: &Alpha) -> HandlerResult {
        let _ = self.started_tx.send("alpha");
        self.release.notified().await;
        Ok(())
    }
}

struct SlowBetaHandler {
    started_tx: mpsc::UnboundedSender<&'static str>,
    release: Arc<Notify>,
}

impl EventHandler<Beta> for SlowBetaHandler {
    async fn handle(&self, _event: &Beta) -> HandlerResult {
        let _ = self.started_tx.send("beta");
        self.release.notified().await;
        Ok(())
    }
}

struct AlphaAsyncCounter(Arc<AtomicUsize>);

impl EventHandler<Alpha> for AlphaAsyncCounter {
    async fn handle(&self, event: &Alpha) -> HandlerResult {
        self.0.fetch_add(event.0, Ordering::SeqCst);
        Ok(())
    }
}

struct BetaAsyncCounter(Arc<AtomicUsize>);

impl EventHandler<Beta> for BetaAsyncCounter {
    async fn handle(&self, event: &Beta) -> HandlerResult {
        self.0.fetch_add(event.0, Ordering::SeqCst);
        Ok(())
    }
}

struct GammaAsyncCounter(Arc<AtomicUsize>);

impl EventHandler<Gamma> for GammaAsyncCounter {
    async fn handle(&self, _event: &Gamma) -> HandlerResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct AlphaSyncCounter(Arc<AtomicUsize>);

impl SyncEventHandler<Alpha> for AlphaSyncCounter {
    fn handle(&self, _event: &Alpha) -> HandlerResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

/// Different event types are dispatched via independent type slots, so two
/// slow handlers on different event types should run concurrently rather
/// than blocking each other.
#[tokio::test]
async fn per_type_parallelism() {
    let bus = EventBus::new(64).expect("valid config");
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let release = Arc::new(Notify::new());

    let _ = bus
        .subscribe(SlowAlphaHandler {
            started_tx: started_tx.clone(),
            release: Arc::clone(&release),
        })
        .await
        .expect("subscribe alpha");

    let _ = bus
        .subscribe(SlowBetaHandler {
            started_tx,
            release: Arc::clone(&release),
        })
        .await
        .expect("subscribe beta");

    // Publish to both types — dispatch returns quickly because handlers
    // are async (spawned).
    bus.publish(Alpha(1)).await.expect("publish alpha");
    bus.publish(Beta(1)).await.expect("publish beta");

    // Both handlers should start before either is released. If type slots were
    // serialized, only one start signal would arrive here.
    let mut seen = std::collections::HashSet::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while seen.len() < 2 {
            let who = started_rx.recv().await.expect("start channel should remain open");
            seen.insert(who);
        }
    })
    .await
    .expect("timed out waiting for both handlers to start");

    assert!(seen.contains("alpha"), "alpha handler did not start");
    assert!(seen.contains("beta"), "beta handler did not start");

    release.notify_waiters();

    bus.shutdown().await.expect("shutdown");
}

/// Events of the same type dispatched sequentially should be processed
/// in FIFO order by the same sync lane.
#[tokio::test]
async fn fifo_within_event_type() {
    let bus = EventBus::new(64).expect("valid config");
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));

    let _ = bus.subscribe(SyncAlphaRecorder { log: Arc::clone(&log) }).await.expect("subscribe");

    for i in 0..10 {
        bus.publish(Alpha(i)).await.expect("publish");
    }

    bus.shutdown().await.expect("shutdown");

    let entries: Vec<usize> = log.lock().unwrap().clone();
    let expected: Vec<usize> = (0..10).collect();
    assert_eq!(entries, expected, "events should arrive in FIFO order within the same event type");
}

/// No type slot should exist until a subscription for that event
/// type is actually registered. We verify this indirectly: registering
/// event types for only Alpha and publishing to Beta should result in no
/// stats for Beta.
#[tokio::test]
async fn lazy_type_slot_registration() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    // Only subscribe to Alpha.
    let _ = bus.subscribe(AlphaSyncCounter(Arc::clone(&count))).await.expect("subscribe alpha");

    let stats = bus.stats().await.expect("stats");
    assert_eq!(stats.registered_event_types.len(), 1, "only one event type should have a slot");

    // Publishing Beta should be a no-op (no slot exists for it).
    bus.publish(Beta(1)).await.expect("publish beta");

    // Stats should still show only one event type.
    let stats = bus.stats().await.expect("stats after publish");
    assert_eq!(
        stats.registered_event_types.len(),
        1,
        "publishing to an unregistered type should not create a slot"
    );

    bus.shutdown().await.expect("shutdown");
    assert_eq!(count.load(Ordering::SeqCst), 0, "Alpha handler should not have fired");
}

/// Unregistering all listeners and middleware for an event type should
/// eventually remove the slot, freeing resources.
#[tokio::test]
async fn type_slot_cleanup_on_empty() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let sub = bus.subscribe(AlphaSyncCounter(Arc::clone(&count))).await.expect("subscribe");

    let stats = bus.stats().await.expect("stats before unsub");
    assert_eq!(stats.total_subscriptions, 1);
    assert_eq!(stats.registered_event_types.len(), 1, "slot should exist for Alpha");

    // Unsubscribe the only listener — slot becomes empty.
    let removed = sub.unsubscribe().await.expect("unsubscribe");
    assert!(removed, "unsubscribe should return true");

    let stats = bus.stats().await.expect("stats after unsub");
    assert_eq!(stats.total_subscriptions, 0);
    assert_eq!(stats.registered_event_types.len(), 0, "empty slot should be cleaned up");

    bus.shutdown().await.expect("shutdown");
}

/// Shutdown must aggregate results from all active type slots.
/// If we have three event types registered, shutdown should drain all
/// of them and report success.
#[tokio::test]
async fn shutdown_aggregates_all_type_slots() {
    let bus = EventBus::new(64).expect("valid config");
    let alpha_count = Arc::new(AtomicUsize::new(0));
    let beta_count = Arc::new(AtomicUsize::new(0));
    let gamma_count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(AlphaAsyncCounter(Arc::clone(&alpha_count))).await.expect("subscribe alpha");

    let _ = bus.subscribe(BetaAsyncCounter(Arc::clone(&beta_count))).await.expect("subscribe beta");

    let _ = bus.subscribe(GammaAsyncCounter(Arc::clone(&gamma_count))).await.expect("subscribe gamma");

    bus.publish(Alpha(10)).await.expect("publish alpha");
    bus.publish(Beta(20)).await.expect("publish beta");
    bus.publish(Gamma).await.expect("publish gamma");

    // Shutdown drains in-flight handlers across all type slots.
    bus.shutdown().await.expect("shutdown");

    assert_eq!(alpha_count.load(Ordering::SeqCst), 10, "alpha handler should have executed");
    assert_eq!(beta_count.load(Ordering::SeqCst), 20, "beta handler should have executed");
    assert_eq!(gamma_count.load(Ordering::SeqCst), 1, "gamma handler should have executed");
}

/// Multiple listeners on the same event type coexist within a single
/// type slot — each receives the event.
#[tokio::test]
async fn multiple_listeners_share_type_slot() {
    let bus = EventBus::new(64).expect("valid config");
    let sync_count = Arc::new(AtomicUsize::new(0));
    let async_count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(AlphaSyncCounter(Arc::clone(&sync_count))).await.expect("subscribe sync");

    let _ = bus.subscribe(AlphaAsyncCounter(Arc::clone(&async_count))).await.expect("subscribe async");

    // Despite two subscriptions, there should be only one event type registered.
    let stats = bus.stats().await.expect("stats");
    assert_eq!(stats.total_subscriptions, 2, "both listeners should be counted");
    assert_eq!(stats.registered_event_types.len(), 1, "both should share a single type slot");

    bus.publish(Alpha(5)).await.expect("publish");
    bus.shutdown().await.expect("shutdown");

    assert_eq!(sync_count.load(Ordering::SeqCst), 1, "sync handler should fire");
    assert_eq!(async_count.load(Ordering::SeqCst), 5, "async handler should fire");
}
