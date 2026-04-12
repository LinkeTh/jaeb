use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{EventBus, EventBusError, EventHandler, HandlerResult};

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Msg;

// ── Handlers ─────────────────────────────────────────────────────────

struct SlowHandler {
    count: Arc<AtomicUsize>,
}

impl EventHandler<Msg> for SlowHandler {
    async fn handle(&self, _event: &Msg) -> HandlerResult {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn try_publish_reports_channel_full() {
    let bus = EventBus::builder().buffer_size(1).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(SlowHandler { count: Arc::clone(&count) }).await.expect("subscribe");

    bus.try_publish(Msg).expect("first try_publish should succeed");

    let err = bus.try_publish(Msg).expect_err("second try_publish should fail");
    assert_eq!(err, EventBusError::ChannelFull);

    bus.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn try_publish_after_shutdown_returns_stopped() {
    let bus = EventBus::builder().buffer_size(16).build().await.expect("valid config");
    bus.shutdown().await.expect("shutdown");

    let err = bus.try_publish(Msg).expect_err("try_publish after shutdown");
    assert_eq!(err, EventBusError::Stopped);
}

/// Fill dispatch permits with `try_publish`, then verify that `publish`
/// (the blocking variant) eventually succeeds once in-flight work completes.
#[tokio::test]
async fn publish_succeeds_after_channel_drains() {
    let bus = EventBus::builder().buffer_size(2).build().await.expect("valid config");
    let count = Arc::new(AtomicUsize::new(0));

    let _ = bus.subscribe(SlowHandler { count: Arc::clone(&count) }).await.expect("subscribe");

    // Fill dispatch permits. try_publish is non-blocking and succeeds as long
    // as immediate capacity is available.
    bus.try_publish(Msg).expect("try_publish 1");
    bus.try_publish(Msg).expect("try_publish 2");

    // Permits should be exhausted now.
    assert_eq!(bus.try_publish(Msg).unwrap_err(), EventBusError::ChannelFull);

    // The blocking `publish` should succeed once one in-flight dispatch
    // completes and frees capacity. Use a timeout to avoid hanging if
    // something goes wrong.
    tokio::time::timeout(std::time::Duration::from_secs(5), bus.publish(Msg))
        .await
        .expect("publish should not time out")
        .expect("publish should succeed after drain");

    bus.shutdown().await.expect("shutdown");

    // All 3 events (2 via try_publish + 1 via publish) should have been
    // handled.
    assert_eq!(count.load(Ordering::SeqCst), 3, "all events should have been processed");
}
