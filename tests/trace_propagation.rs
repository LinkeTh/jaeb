//! Tests that verify tracing span context propagation from publish site to async handlers.
//!
//! When the `trace` feature is enabled, the event bus captures `Span::current()`
//! at publish time and instruments spawned async handler futures so they inherit
//! the caller's span context automatically.
//!
//! These tests require the `trace` feature to be active.
#![cfg(feature = "trace")]

use std::sync::Arc;
use std::time::Duration;

use jaeb::{EventBus, EventHandler, HandlerResult, SyncEventHandler};
use tokio::sync::Notify;
use tracing::Span;

// ── Event types ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct TracedEvent {
    #[allow(dead_code)]
    value: usize,
}

// ── Handlers that capture the active span ────────────────────────────

struct SpanCapturingAsyncHandler {
    captured: Arc<tokio::sync::Mutex<Option<tracing::Id>>>,
    done: Arc<Notify>,
}

impl EventHandler<TracedEvent> for SpanCapturingAsyncHandler {
    async fn handle(&self, _event: &TracedEvent, _bus: &EventBus) -> HandlerResult {
        // Record the span ID that is current when the handler runs.
        let current = Span::current();
        let id = current.id();
        *self.captured.lock().await = id;
        self.done.notify_one();
        Ok(())
    }
}

struct SpanCapturingSyncHandler {
    captured: Arc<std::sync::Mutex<Option<tracing::Id>>>,
}

impl SyncEventHandler<TracedEvent> for SpanCapturingSyncHandler {
    fn handle(&self, _event: &TracedEvent, _bus: &EventBus) -> HandlerResult {
        let current = Span::current();
        let id = current.id();
        *self.captured.lock().expect("lock") = id;
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

/// Verify that an async handler spawned by publish inherits the caller's
/// tracing span, so trace context propagates without user intervention.
#[tokio::test]
async fn async_handler_inherits_publish_span() {
    // Install a subscriber so span IDs are actually allocated.
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_test_writer()
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let bus = EventBus::builder().build().await.expect("valid config");
    let captured = Arc::new(tokio::sync::Mutex::new(None));
    let done = Arc::new(Notify::new());

    let _ = bus
        .subscribe(SpanCapturingAsyncHandler {
            captured: Arc::clone(&captured),
            done: Arc::clone(&done),
        })
        .await
        .expect("subscribe");

    // Publish inside a named span.
    let outer_span = tracing::info_span!("test.outer");
    let outer_id = outer_span.id();
    assert!(outer_id.is_some(), "subscriber must allocate span IDs for this test");

    {
        let _entered = outer_span.enter();
        bus.publish(TracedEvent { value: 1 }).await.expect("publish");
    }

    // Wait for handler to complete.
    tokio::time::timeout(Duration::from_secs(2), done.notified())
        .await
        .expect("handler should complete within timeout");

    bus.shutdown().await.expect("shutdown");

    let handler_span_id = captured.lock().await.clone();
    // The handler should run within the outer span (same span ID or a child).
    // With `Instrument`, the future is instrumented with the *exact* captured
    // span, so the handler sees that span as current.
    assert!(handler_span_id.is_some(), "handler should have an active span (not None)");
    assert_eq!(handler_span_id, outer_id, "async handler should inherit the publisher's span");
}

/// Verify that sync handlers naturally see the caller's span (no special
/// instrumentation needed — they run inline on the publish task).
#[tokio::test]
async fn sync_handler_sees_publish_span() {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_test_writer()
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let bus = EventBus::builder().build().await.expect("valid config");
    let captured = Arc::new(std::sync::Mutex::new(None));

    let _ = bus
        .subscribe(SpanCapturingSyncHandler {
            captured: Arc::clone(&captured),
        })
        .await
        .expect("subscribe");

    let outer_span = tracing::info_span!("test.sync_outer");
    let outer_id = outer_span.id();

    {
        let _entered = outer_span.enter();
        bus.publish(TracedEvent { value: 2 }).await.expect("publish");
    }

    bus.shutdown().await.expect("shutdown");

    let handler_span_id = captured.lock().expect("lock").clone();
    assert!(handler_span_id.is_some(), "sync handler should have an active span");
    assert_eq!(handler_span_id, outer_id, "sync handler should see the publisher's span");
}

/// Verify that when no span is active at publish time, the handler still
/// runs correctly (with no span / the root context).
#[tokio::test]
async fn async_handler_works_without_active_span() {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_test_writer()
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let bus = EventBus::builder().build().await.expect("valid config");
    let captured = Arc::new(tokio::sync::Mutex::new(None));
    let done = Arc::new(Notify::new());

    let _ = bus
        .subscribe(SpanCapturingAsyncHandler {
            captured: Arc::clone(&captured),
            done: Arc::clone(&done),
        })
        .await
        .expect("subscribe");

    // Publish without entering any span — Span::current() is the disabled/noop span.
    bus.publish(TracedEvent { value: 3 }).await.expect("publish");

    tokio::time::timeout(Duration::from_secs(2), done.notified())
        .await
        .expect("handler should complete within timeout");

    bus.shutdown().await.expect("shutdown");

    // Handler should still work. The captured span ID will be None (noop span).
    let handler_span_id = captured.lock().await.clone();
    assert!(handler_span_id.is_none(), "handler should have no active span when publisher had none");
}

/// Verify span propagation for the single-async-listener path. This covers the
/// optimized one-listener scheduling branch used by dispatch and ensures the
/// publisher's span still reaches the async handler.
#[tokio::test]
async fn single_async_listener_path_propagates_caller_span() {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_test_writer()
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // A non-once async handler with a single listener exercises the single
    // listener dispatch path.
    let bus = EventBus::builder().build().await.expect("valid config");
    let captured = Arc::new(tokio::sync::Mutex::new(None));
    let done = Arc::new(Notify::new());

    let _ = bus
        .subscribe(SpanCapturingAsyncHandler {
            captured: Arc::clone(&captured),
            done: Arc::clone(&done),
        })
        .await
        .expect("subscribe");

    let outer_span = tracing::info_span!("test.worker_path");
    let outer_id = outer_span.id();

    // Publish multiple times to ensure the worker processes at least one
    // with the span active. Only the last publish's span is captured.
    {
        let _entered = outer_span.enter();
        bus.publish(TracedEvent { value: 5 }).await.expect("publish");
    }

    tokio::time::timeout(Duration::from_secs(2), done.notified())
        .await
        .expect("handler should complete within timeout");

    bus.shutdown().await.expect("shutdown");

    let handler_span_id = captured.lock().await.clone();
    assert!(handler_span_id.is_some(), "handler via single-listener path should have an active span");
    assert_eq!(
        handler_span_id, outer_id,
        "single-listener path should propagate the publisher's span to async handlers"
    );
}
