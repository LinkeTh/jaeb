use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jaeb::{EventBus, EventHandler, HandlerResult};

#[derive(Clone, Debug)]
struct Ping;

#[derive(Clone, Debug)]
struct Pong;

// Publishes Pong from inside the async handle method.
struct PingHandler;

impl EventHandler<Ping> for PingHandler {
    async fn handle(&self, _event: &Ping, bus: &EventBus) -> HandlerResult {
        bus.publish(Pong).await.map_err(|e| e.to_string())?;
        Ok(())
    }
}

struct PongHandler {
    count: Arc<AtomicUsize>,
    notify: Arc<tokio::sync::Notify>,
}

impl EventHandler<Pong> for PongHandler {
    async fn handle(&self, _event: &Pong, _bus: &EventBus) -> HandlerResult {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.notify.notify_one();
        Ok(())
    }
}

#[tokio::test]
async fn async_handler_can_publish_to_bus() {
    let bus = EventBus::builder().build().await.expect("valid config");
    let pong_count = Arc::new(AtomicUsize::new(0));
    let notified = Arc::new(tokio::sync::Notify::new());

    let _ = bus.subscribe(PingHandler).await.expect("subscribe ping");
    let _ = bus
        .subscribe(PongHandler {
            count: Arc::clone(&pong_count),
            notify: Arc::clone(&notified),
        })
        .await
        .expect("subscribe pong");

    bus.publish(Ping).await.expect("publish ping");

    // Wait for PongHandler to fire before shutting down.
    tokio::time::timeout(std::time::Duration::from_secs(2), notified.notified())
        .await
        .expect("PongHandler did not fire within 2s");

    bus.shutdown().await.expect("shutdown");

    assert_eq!(pong_count.load(Ordering::SeqCst), 1, "pong handler should have fired once");
}

#[tokio::test]
async fn async_closure_handler_can_publish_to_bus() {
    let bus = EventBus::builder().build().await.expect("valid config");
    let pong_count = Arc::new(AtomicUsize::new(0));
    let notified = Arc::new(tokio::sync::Notify::new());

    let pong_count_clone = Arc::clone(&pong_count);
    let notify_clone = Arc::clone(&notified);
    let _ = bus
        .subscribe(move |_event: Pong, _bus: EventBus| {
            let count = Arc::clone(&pong_count_clone);
            let notify = Arc::clone(&notify_clone);
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                notify.notify_one();
                Ok(())
            }
        })
        .await
        .expect("subscribe pong closure");

    let _ = bus
        .subscribe(|_event: Ping, bus: EventBus| async move {
            bus.publish(Pong).await.map_err(|e: jaeb::EventBusError| e.to_string())?;
            Ok(())
        })
        .await
        .expect("subscribe ping closure");

    bus.publish(Ping).await.expect("publish ping");

    // Wait for the pong closure handler to fire before shutting down.
    tokio::time::timeout(std::time::Duration::from_secs(2), notified.notified())
        .await
        .expect("pong closure handler did not fire within 2s");

    bus.shutdown().await.expect("shutdown");

    assert_eq!(pong_count.load(Ordering::SeqCst), 1, "pong closure handler should have fired once");
}

#[cfg(feature = "macros")]
mod macro_tests {
    use super::*;
    use jaeb::handler;

    #[handler]
    async fn on_ping(_event: &Ping, bus: &EventBus) -> HandlerResult {
        bus.publish(Pong).await.map_err(|e| e.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn macro_handler_can_use_bus() {
        let bus = EventBus::builder().handler(on_ping).build().await.expect("valid config");

        let pong_count = Arc::new(AtomicUsize::new(0));
        let notified = Arc::new(tokio::sync::Notify::new());

        let _ = bus
            .subscribe(PongHandler {
                count: Arc::clone(&pong_count),
                notify: Arc::clone(&notified),
            })
            .await
            .expect("subscribe pong");

        bus.publish(Ping).await.expect("publish ping");

        // Wait for PongHandler to fire before shutting down.
        tokio::time::timeout(std::time::Duration::from_secs(2), notified.notified())
            .await
            .expect("PongHandler did not fire within 2s");

        bus.shutdown().await.expect("shutdown");

        assert_eq!(
            pong_count.load(Ordering::SeqCst),
            1,
            "pong handler should have fired once via #[handler] macro"
        );
    }
}
