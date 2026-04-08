// SPDX-License-Identifier: MIT
//
// summer-jaeb-demo: demonstrates the SummerJaeb plugin within a summer-rs web app.
//
// Flow:
//   1. SummerJaeb reads [jaeb] config, builds the EventBus, registers it as a component.
//   2. The #[component] setup_handlers function depends on SummerJaeb, retrieves the bus
//      via DI, and subscribes event handlers at startup.
//   3. HTTP endpoints use Component<EventBus> to publish events when requests arrive.
//   4. Handlers react to the events in the background.
//
// OpenAPI docs are served at /docs (Scalar UI).

use jaeb::{EventBus, EventHandler, HandlerResult, SyncEventHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use summer::auto_config;
use summer::extractor::Component;
use summer::App;
use summer_jaeb::SummerJaeb;
use summer_web::axum::Json;
use summer_web::extractor::Component as WebComponent;
use summer_web::extractor::Path;
use summer_web::{post_api, WebConfigurator, WebPlugin};
use tracing::info;

// ── Events ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct OrderPlacedEvent {
    order_id: u32,
}

#[derive(Clone, Debug)]
struct OrderShippedEvent {
    order_id: u32,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// Async handler: reacts to a new order being placed.
struct OnOrderPlaced;

impl EventHandler<OrderPlacedEvent> for OnOrderPlaced {
    async fn handle(&self, event: &OrderPlacedEvent) -> HandlerResult {
        info!(order_id = event.order_id, "order placed — sending confirmation email");
        Ok(())
    }
}

/// Sync handler: reacts to an order being shipped.
struct OnOrderShipped;

impl SyncEventHandler<OrderShippedEvent> for OnOrderShipped {
    fn handle(&self, event: &OrderShippedEvent) -> HandlerResult {
        info!(order_id = event.order_id, "order shipped — updating inventory");
        Ok(())
    }
}

// ── Handler subscription via #[component] ────────────────────────────────────

/// Marker type returned by setup_handlers (the #[component] macro requires a return value).
#[derive(Clone)]
struct HandlerSetup;

/// Subscribes event handlers at startup. The #[inject("SummerJaeb")] attribute tells
/// the macro to declare a dependency on the SummerJaeb plugin so the EventBus is
/// available when this component function runs.
#[summer::component]
async fn setup_handlers(
    #[inject("SummerJaeb")] Component(bus): Component<EventBus>,
) -> HandlerSetup {
    bus.subscribe(OnOrderPlaced)
        .await
        .expect("failed to subscribe OnOrderPlaced");

    bus.subscribe(OnOrderShipped)
        .await
        .expect("failed to subscribe OnOrderShipped");

    HandlerSetup
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
async fn place_order(
    WebComponent(bus): WebComponent<EventBus>,
    Json(body): Json<PlaceOrderRequest>,
) -> Json<OrderResponse> {
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
async fn ship_order(
    WebComponent(bus): WebComponent<EventBus>,
    Path(id): Path<u32>,
) -> Json<OrderResponse> {
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
        .add_plugin(SummerJaeb)
        .add_plugin(WebPlugin)
        .run()
        .await;
}
