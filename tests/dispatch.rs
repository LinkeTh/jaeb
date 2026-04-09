// SPDX-License-Identifier: MIT
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{EventBus, EventHandler, HandlerResult, SyncEventHandler};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Ping {
    value: usize,
}

#[derive(Clone, Debug)]
struct Pong {
    value: usize,
}

// ── Handlers ─────────────────────────────────────────────────────────

struct AsyncCounter {
    count: Arc<AtomicUsize>,
}

impl EventHandler<Ping> for AsyncCounter {
    async fn handle(&self, event: &Ping) -> HandlerResult {
        self.count.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

struct SyncCounter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<Ping> for SyncCounter {
    fn handle(&self, event: &Ping) -> HandlerResult {
        self.count.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

struct PongCounter {
    count: Arc<AtomicUsize>,
}

impl SyncEventHandler<Pong> for PongCounter {
    fn handle(&self, event: &Pong) -> HandlerResult {
        self.count.fetch_add(event.value, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn async_handler_receives_event() {
    let bus = EventBus::new(16);
    let count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(AsyncCounter { count: Arc::clone(&count) }).await.expect("subscribe");

    bus.publish(Ping { value: 42 }).await.expect("publish");

    // Shutdown drains in-flight async handlers deterministically.
    bus.shutdown().await.expect("shutdown");

    assert_eq!(count.load(Ordering::SeqCst), 42);
}

#[tokio::test]
async fn sync_handler_receives_event() {
    let bus = EventBus::new(16);
    let count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(SyncCounter { count: Arc::clone(&count) }).await.expect("subscribe");

    bus.publish(Ping { value: 7 }).await.expect("publish");

    // Sync handlers complete before publish returns.
    assert_eq!(count.load(Ordering::SeqCst), 7);
    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn publish_with_no_listeners_is_noop() {
    let bus = EventBus::new(16);

    // Publishing with no registered handlers should not error or panic.
    bus.publish(Ping { value: 99 }).await.expect("publish with no listeners");

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn multiple_handlers_same_event_all_receive() {
    let bus = EventBus::new(16);
    let sync_count = Arc::new(AtomicUsize::new(0));
    let async_count_a = Arc::new(AtomicUsize::new(0));
    let async_count_b = Arc::new(AtomicUsize::new(0));

    let _ = bus
        .subscribe(SyncCounter {
            count: Arc::clone(&sync_count),
        })
        .await
        .expect("subscribe sync");

    let _ = bus
        .subscribe(AsyncCounter {
            count: Arc::clone(&async_count_a),
        })
        .await
        .expect("subscribe async a");

    let _ = bus
        .subscribe(AsyncCounter {
            count: Arc::clone(&async_count_b),
        })
        .await
        .expect("subscribe async b");

    bus.publish(Ping { value: 5 }).await.expect("publish");

    // Shutdown drains in-flight async handlers deterministically.
    bus.shutdown().await.expect("shutdown");

    assert_eq!(sync_count.load(Ordering::SeqCst), 5);
    assert_eq!(async_count_a.load(Ordering::SeqCst), 5);
    assert_eq!(async_count_b.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn handlers_only_receive_their_event_type() {
    let bus = EventBus::new(16);
    let ping_count = Arc::new(AtomicUsize::new(0));
    let pong_count = Arc::new(AtomicUsize::new(0));

    let _ = bus
        .subscribe(SyncCounter {
            count: Arc::clone(&ping_count),
        })
        .await
        .expect("subscribe ping");

    let _ = bus
        .subscribe(PongCounter {
            count: Arc::clone(&pong_count),
        })
        .await
        .expect("subscribe pong");

    // Only publish Ping — Pong handler must not fire.
    bus.publish(Ping { value: 10 }).await.expect("publish ping");

    assert_eq!(ping_count.load(Ordering::SeqCst), 10);
    assert_eq!(pong_count.load(Ordering::SeqCst), 0);

    bus.shutdown().await.expect("shutdown");
}
