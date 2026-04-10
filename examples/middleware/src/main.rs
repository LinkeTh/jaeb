//! Middleware: global and typed pre-dispatch filtering.
//!
//! Global middleware sees every event. Typed middleware only runs for its
//! target event type. Rejection short-circuits the pipeline.

use std::any::Any;

use jaeb::{EventBus, EventBusError, HandlerResult, MiddlewareDecision, SyncEventHandler, SyncMiddleware, TypedSyncMiddleware};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Order {
    id: u32,
    vip: bool,
}

#[derive(Clone, Debug)]
struct Metric {
    name: &'static str,
}

// ── Middleware ───────────────────────────────────────────────────────────

/// Global: logs every event that passes through the bus.
struct LogAllMiddleware;

impl SyncMiddleware for LogAllMiddleware {
    fn process(&self, event_name: &'static str, _event: &(dyn Any + Send + Sync)) -> MiddlewareDecision {
        println!("[middleware] global: saw event {event_name}");
        MiddlewareDecision::Continue
    }
}

/// Typed: rejects non-VIP orders.
struct VipOnlyMiddleware;

impl TypedSyncMiddleware<Order> for VipOnlyMiddleware {
    fn process(&self, _event_name: &'static str, event: &Order) -> MiddlewareDecision {
        if event.vip {
            MiddlewareDecision::Continue
        } else {
            MiddlewareDecision::Reject(format!("order {} is not VIP", event.id))
        }
    }
}

// ── Handlers ────────────────────────────────────────────────────────────

struct OrderHandler;
impl SyncEventHandler<Order> for OrderHandler {
    fn handle(&self, event: &Order) -> HandlerResult {
        println!("handler: processing order {}", event.id);
        Ok(())
    }
}

struct MetricHandler;
impl SyncEventHandler<Metric> for MetricHandler {
    fn handle(&self, event: &Metric) -> HandlerResult {
        println!("handler: recorded metric {}", event.name);
        Ok(())
    }
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let bus = EventBus::new(64).expect("valid config");

    let _ = bus.add_sync_middleware(LogAllMiddleware).await.expect("add global middleware failed");
    let _ = bus
        .add_typed_sync_middleware::<Order, _>(VipOnlyMiddleware)
        .await
        .expect("add typed middleware failed");

    let _ = bus.subscribe::<Order, _, _>(OrderHandler).await.expect("subscribe failed");
    let _ = bus.subscribe::<Metric, _, _>(MetricHandler).await.expect("subscribe failed");

    // VIP order: passes both middlewares.
    bus.publish(Order { id: 1, vip: true }).await.expect("publish failed");

    // Non-VIP order: rejected by typed middleware.
    match bus.publish(Order { id: 2, vip: false }).await {
        Err(EventBusError::MiddlewareRejected(reason)) => {
            println!("rejected: {reason}");
        }
        other => panic!("expected MiddlewareRejected, got {other:?}"),
    }

    // Metric event: only global middleware runs (typed middleware is scoped to Order).
    bus.publish(Metric { name: "cpu_usage" }).await.expect("publish failed");

    bus.shutdown().await.expect("shutdown failed");
}
