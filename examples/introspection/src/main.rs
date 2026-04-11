//! Introspection: handler `name()`, `BusStats`, `stats()`, and `is_healthy()`.

use jaeb::{EventBus, EventHandler, HandlerResult, SyncEventHandler};

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct EventA;

#[derive(Clone, Debug)]
struct EventB;

// ── Handlers ────────────────────────────────────────────────────────────

struct HandlerA;
impl EventHandler<EventA> for HandlerA {
    async fn handle(&self, _event: &EventA) -> HandlerResult {
        Ok(())
    }
    fn name(&self) -> Option<&'static str> {
        Some("handler-a")
    }
}

struct HandlerB;
impl SyncEventHandler<EventB> for HandlerB {
    fn handle(&self, _event: &EventB) -> HandlerResult {
        Ok(())
    }
    fn name(&self) -> Option<&'static str> {
        Some("handler-b")
    }
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let bus = EventBus::new(64).expect("valid config");

    let _ = bus.subscribe::<EventA, _, _>(HandlerA).await.expect("subscribe failed");
    let _ = bus.subscribe::<EventB, _, _>(HandlerB).await.expect("subscribe failed");

    // stats() returns a point-in-time snapshot of the bus state.
    let stats = bus.stats().await.expect("stats failed");
    println!("total subscriptions: {}", stats.total_subscriptions);
    println!("registered event types: {:?}", stats.registered_event_types);
    println!("queue capacity: {}", stats.queue_capacity);
    println!("publish permits available: {}", stats.publish_permits_available);
    println!("publish in-flight: {}", stats.publish_in_flight);
    println!("in-flight async: {}", stats.in_flight_async);

    // Per-event-type details include listener names.
    for (event_type, listeners) in &stats.subscriptions_by_event {
        for listener in listeners {
            println!("  [{event_type}] id={}, name={:?}", listener.subscription_id, listener.name);
        }
    }

    // is_healthy() checks the bus is running and the control loop is alive.
    println!("healthy: {}", bus.is_healthy().await);

    bus.shutdown().await.expect("shutdown failed");

    // After shutdown, is_healthy returns false and stats returns Err(Stopped).
    println!("healthy after shutdown: {}", bus.is_healthy().await);
    assert!(bus.stats().await.is_err());

    println!("done");
}
