//! Graceful shutdown: drain in-flight tasks, abort on timeout.
//!
//! `shutdown()` waits for async handlers to complete. When a
//! `shutdown_timeout` is configured, remaining tasks are aborted at
//! the deadline and `ShutdownTimeout` is returned.

use std::time::Duration;

use jaeb::{EventBus, EventBusError, EventHandler, HandlerResult};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct QuickJob;

#[derive(Clone, Debug)]
struct SlowJob;

// ── Handlers ────────────────────────────────────────────────────────────

struct QuickHandler;
impl EventHandler<QuickJob> for QuickHandler {
    async fn handle(&self, _event: &QuickJob) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(50)).await;
        println!("quick job completed");
        Ok(())
    }
}

struct VerySlowHandler;
impl EventHandler<SlowJob> for VerySlowHandler {
    async fn handle(&self, _event: &SlowJob) -> HandlerResult {
        tokio::time::sleep(Duration::from_secs(5)).await;
        println!("slow job completed");
        Ok(())
    }
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Scenario A: handlers finish before the timeout — clean drain.
    {
        let bus = EventBus::builder()
            .buffer_size(64)
            .shutdown_timeout(Duration::from_secs(2))
            .build()
            .expect("valid config");

        let _ = bus.subscribe::<QuickJob, _, _>(QuickHandler).await.expect("subscribe failed");
        bus.publish(QuickJob).await.expect("publish failed");

        match bus.shutdown().await {
            Ok(()) => println!("scenario A: drained cleanly"),
            Err(e) => println!("scenario A: unexpected error: {e}"),
        }
    }

    // Scenario B: handler exceeds timeout — bus returns ShutdownTimeout.
    {
        let bus = EventBus::builder()
            .buffer_size(64)
            .shutdown_timeout(Duration::from_millis(200))
            .build()
            .expect("valid config");

        let _ = bus.subscribe::<SlowJob, _, _>(VerySlowHandler).await.expect("subscribe failed");
        bus.publish(SlowJob).await.expect("publish failed");

        match bus.shutdown().await {
            Err(EventBusError::ShutdownTimeout) => println!("scenario B: timed out (expected)"),
            other => println!("scenario B: unexpected result: {other:?}"),
        }
    }

    // After shutdown, further publishes return Stopped.
    let bus = EventBus::new(64).expect("valid config");
    bus.shutdown().await.expect("shutdown failed");
    match bus.publish(QuickJob).await {
        Err(EventBusError::Stopped) => println!("post-shutdown publish: Stopped (expected)"),
        other => println!("unexpected: {other:?}"),
    }
}
