//! Tests for once-off (fire-once) subscriptions.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::SyncSubscriptionPolicy;
use jaeb::{DeadLetter, EventBus, EventHandler, HandlerResult, SyncEventHandler};

#[derive(Clone)]
struct Ping;

// ---------------------------------------------------------------------------
// Sync: fires exactly once
// ---------------------------------------------------------------------------

struct SyncCounter(Arc<AtomicUsize>);

impl SyncEventHandler<Ping> for SyncCounter {
    fn handle(&self, _event: &Ping) -> HandlerResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn once_sync_handler_fires_once() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _sub = bus
        .subscribe_once::<Ping, _, _>(SyncCounter(Arc::clone(&count)))
        .await
        .expect("subscribe_once");

    // Publish three times — handler should only fire for the first.
    bus.publish(Ping).await.expect("publish 1");
    bus.publish(Ping).await.expect("publish 2");
    bus.publish(Ping).await.expect("publish 3");

    bus.shutdown().await.expect("shutdown");
    assert_eq!(count.load(Ordering::SeqCst), 1, "sync once-handler should fire exactly once");
}

// ---------------------------------------------------------------------------
// Async: fires exactly once
// ---------------------------------------------------------------------------

struct AsyncCounter(Arc<AtomicUsize>);

impl EventHandler<Ping> for AsyncCounter {
    async fn handle(&self, _event: &Ping) -> HandlerResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn once_async_handler_fires_once() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _sub = bus
        .subscribe_once::<Ping, _, _>(AsyncCounter(Arc::clone(&count)))
        .await
        .expect("subscribe_once");

    bus.publish(Ping).await.expect("publish 1");
    bus.publish(Ping).await.expect("publish 2");
    bus.publish(Ping).await.expect("publish 3");

    bus.shutdown().await.expect("shutdown");
    assert_eq!(count.load(Ordering::SeqCst), 1, "async once-handler should fire exactly once");
}

// ---------------------------------------------------------------------------
// Failure still removes + dead letter emitted
// ---------------------------------------------------------------------------

struct FailingHandler;

impl SyncEventHandler<Ping> for FailingHandler {
    fn handle(&self, _event: &Ping) -> HandlerResult {
        Err("intentional failure".into())
    }
}

struct DeadLetterCounter(Arc<AtomicUsize>);

impl SyncEventHandler<DeadLetter> for DeadLetterCounter {
    fn handle(&self, _event: &DeadLetter) -> HandlerResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn once_handler_failure_emits_dead_letter_and_removes() {
    let bus = EventBus::new(64).expect("valid config");
    let dl_count = Arc::new(AtomicUsize::new(0));

    let _dl_sub = bus
        .subscribe_dead_letters(DeadLetterCounter(Arc::clone(&dl_count)))
        .await
        .expect("subscribe dead letters");

    let policy = SyncSubscriptionPolicy::default();
    let _sub = bus
        .subscribe_once_with_policy::<Ping, _, _>(FailingHandler, policy)
        .await
        .expect("subscribe_once_with_policy");

    // First publish triggers the handler, which fails → dead letter.
    bus.publish(Ping).await.expect("publish 1");
    // Second publish should NOT reach the (removed) handler.
    bus.publish(Ping).await.expect("publish 2");

    bus.shutdown().await.expect("shutdown");

    assert_eq!(
        dl_count.load(Ordering::SeqCst),
        1,
        "exactly one dead letter should be emitted (handler fired once, then removed)"
    );
}

// ---------------------------------------------------------------------------
// Unsubscribe after auto-removal is idempotent (returns false)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn once_handler_unsubscribe_after_removal_returns_false() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let sub = bus
        .subscribe_once::<Ping, _, _>(SyncCounter(Arc::clone(&count)))
        .await
        .expect("subscribe_once");

    // Fire and auto-remove.
    bus.publish(Ping).await.expect("publish");

    // Manual unsubscribe should report "not found" since already removed.
    let removed = sub.unsubscribe().await.expect("unsubscribe");
    assert!(!removed, "unsubscribe after auto-removal should return false");

    bus.shutdown().await.expect("shutdown");
}

// ---------------------------------------------------------------------------
// Stats reflect once-listener before and after firing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn once_handler_visible_in_stats_until_fired() {
    let bus = EventBus::new(64).expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _sub = bus
        .subscribe_once::<Ping, _, _>(SyncCounter(Arc::clone(&count)))
        .await
        .expect("subscribe_once");

    // Before firing — should appear in stats.
    let stats = bus.stats().await.expect("stats before");
    assert_eq!(stats.total_subscriptions, 1, "once-listener should appear before firing");

    // Fire the handler.
    bus.publish(Ping).await.expect("publish");

    // After firing — should be gone.
    let stats = bus.stats().await.expect("stats after");
    assert_eq!(stats.total_subscriptions, 0, "once-listener should be removed after firing");

    bus.shutdown().await.expect("shutdown");
}
