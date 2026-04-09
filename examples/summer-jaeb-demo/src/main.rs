// SPDX-License-Identifier: MIT
//
// summer-jaeb-demo: demonstrates the SummerJaeb plugin within a summer-rs web app.
//
// Features showcased:
//   - Stateful listener with `Component<DbPool>` state injection
//   - Failure policy attributes (`retries`, `retry_delay_ms`, `dead_letter`)
//   - Dead-letter listener (auto-detected `DeadLetter` event type)
//   - Async and sync listeners
//
// Flow:
//   1. DbPoolPlugin registers a dummy DbPool as a summer component.
//   2. SummerJaeb reads [jaeb] config, builds the EventBus, registers it as a component.
//   3. All #[event_listener] functions are auto-discovered via inventory and subscribed
//      to the bus during plugin startup — no manual setup_handlers component needed.
//   4. HTTP endpoints use Component<EventBus> to publish events when requests arrive.
//   5. Handlers react to the events in the background.
//
// OpenAPI docs are served at /docs (Scalar UI).

use jaeb::{DeadLetter, EventBus, HandlerResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use summer::app::AppBuilder;
use summer::async_trait;
use summer::auto_config;
use summer::extractor::Component;
use summer::plugin::{MutableComponentRegistry, Plugin};
use summer::App;
use summer_jaeb::{event_listener, SummerJaeb};
use summer_web::axum::Json;
use summer_web::extractor::Component as WebComponent;
use summer_web::extractor::Path;
use summer_web::{post_api, WebConfigurator, WebPlugin};
use tracing::{info, warn};

// ── Events ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct OrderPlacedEvent {
    order_id: u32,
}

#[derive(Clone, Debug)]
struct OrderShippedEvent {
    order_id: u32,
}

// ── Dummy component (simulates a database pool) ─────────────────────────────

/// A dummy database pool for demonstration purposes.
/// In a real app this would be `PgPool`, `sea_orm::DatabaseConnection`, etc.
#[derive(Clone, Debug)]
struct DbPool;

impl DbPool {
    fn log_order(&self, order_id: u32) {
        info!(order_id, "DbPool: persisted order to database");
    }
}

/// Plugin that registers the `DbPool` component.
/// Must be added **before** `SummerJaeb` so the pool is available when
/// `#[event_listener]` functions that depend on `Component<DbPool>` are registered.
struct DbPoolPlugin;

#[async_trait]
impl Plugin for DbPoolPlugin {
    fn name(&self) -> &str {
        "DbPoolPlugin"
    }

    async fn build(&self, app: &mut AppBuilder) {
        app.add_component(DbPool);
        info!("DbPoolPlugin: registered dummy DbPool component");
    }
}

// ── Listeners (auto-registered by SummerJaeb plugin) ─────────────────────────

/// Async listener with state injection: reacts to a new order being placed.
/// The `Component<DbPool>` parameter is automatically resolved from summer's
/// component registry at listener registration time.
#[event_listener(retries = 2, retry_delay_ms = 500, dead_letter = true)]
async fn on_order_placed(event: &OrderPlacedEvent, Component(db): Component<DbPool>) -> HandlerResult {
    db.log_order(event.order_id);
    info!(order_id = event.order_id, "order placed — confirmation email sent");
    Ok(())
}

/// Sync listener: reacts to an order being shipped.
/// Sync handlers execute exactly once — retries are only available for async
/// handlers. On failure, a dead letter is emitted (enabled by default).
#[event_listener]
fn on_order_shipped(event: &OrderShippedEvent) -> HandlerResult {
    info!(order_id = event.order_id, "order shipped — updating inventory");
    Ok(())
}

/// Dead-letter listener: logs events that failed all retry attempts.
/// Must be sync — the macro enforces this because `subscribe_dead_letters`
/// requires `SyncEventHandler`. The `DeadLetter` event type is auto-detected,
/// so `subscribe_dead_letters()` is used instead of `subscribe()`.
#[event_listener]
fn on_dead_letter(event: &DeadLetter) -> HandlerResult {
    warn!(
        event = event.event_name,
        subscription = %event.subscription_id,
        attempts = event.attempts,
        error = %event.error,
        "dead letter received"
    );
    Ok(())
}

// ── HTTP request / response types ────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
struct PlaceOrderRequest {
    order_id: u32,
}

#[derive(Serialize, JsonSchema)]
struct OrderResponse {
    order_id: u32,
    status: String,
}

// ── HTTP endpoints ───────────────────────────────────────────────────────────

/// Place a new order
///
/// Publishes an `OrderPlacedEvent` on the event bus.
///
/// @tag Orders
#[post_api("/orders")]
async fn place_order(WebComponent(bus): WebComponent<EventBus>, Json(body): Json<PlaceOrderRequest>) -> Json<OrderResponse> {
    bus.publish(OrderPlacedEvent { order_id: body.order_id })
        .await
        .expect("failed to publish OrderPlacedEvent");

    Json(OrderResponse {
        order_id: body.order_id,
        status: "placed".into(),
    })
}

/// Ship an order
///
/// Publishes an `OrderShippedEvent` on the event bus.
///
/// @tag Orders
#[post_api("/orders/{id}/ship")]
async fn ship_order(WebComponent(bus): WebComponent<EventBus>, Path(id): Path<u32>) -> Json<OrderResponse> {
    bus.publish(OrderShippedEvent { order_id: id })
        .await
        .expect("failed to publish OrderShippedEvent");

    Json(OrderResponse {
        order_id: id,
        status: "shipped".into(),
    })
}

// ── Entry point ──────────────────────────────────────────────────────────────

#[auto_config(WebConfigurator)]
#[tokio::main]
async fn main() {
    App::new()
        .add_plugin(DbPoolPlugin)
        .add_plugin(SummerJaeb)
        .add_plugin(WebPlugin)
        .run()
        .await;
}
