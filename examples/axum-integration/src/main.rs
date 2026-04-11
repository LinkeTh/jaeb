use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use jaeb::{DeadLetter, EventBus, EventBusError, EventHandler, HandlerResult, SubscriptionPolicy, SyncEventHandler, SyncSubscriptionPolicy};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

#[derive(Clone, Debug)]
struct OrderCreated {
    order_id: String,
    customer_email: String,
    total_cents: u64,
}

#[derive(Debug, Deserialize)]
struct CreateOrderRequest {
    order_id: String,
    customer_email: String,
    total_cents: u64,
}

#[derive(Debug, Serialize)]
struct CreateOrderResponse {
    status: &'static str,
    order_id: String,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    healthy: bool,
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

#[derive(Clone)]
struct AppState {
    bus: EventBus,
}

struct NotificationHandler;

impl EventHandler<OrderCreated> for NotificationHandler {
    async fn handle(&self, event: &OrderCreated) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(10)).await;
        info!(
            order_id = %event.order_id,
            customer_email = %event.customer_email,
            "notification sent"
        );
        Ok(())
    }
}

struct InventoryProjectionHandler;

impl EventHandler<OrderCreated> for InventoryProjectionHandler {
    async fn handle(&self, event: &OrderCreated) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(5)).await;
        info!(order_id = %event.order_id, "inventory projection updated");
        Ok(())
    }
}

struct AuditLogHandler;

impl SyncEventHandler<OrderCreated> for AuditLogHandler {
    fn handle(&self, event: &OrderCreated) -> HandlerResult {
        info!(
            order_id = %event.order_id,
            total_cents = event.total_cents,
            "audit log appended"
        );
        Ok(())
    }
}

struct DeadLetterLogger;

impl SyncEventHandler<DeadLetter> for DeadLetterLogger {
    fn handle(&self, dl: &DeadLetter) -> HandlerResult {
        warn!(
            event = dl.event_name,
            subscription_id = dl.subscription_id.as_u64(),
            attempts = dl.attempts,
            error = %dl.error,
            "dead letter received"
        );
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,jaeb=trace,axum_integration=debug".into()))
        .init();

    let bus = EventBus::builder()
        .buffer_size(256)
        .max_concurrent_async(64)
        .default_subscription_policy(SubscriptionPolicy::default().with_priority(0))
        .build()
        .expect("valid bus config");

    register_handlers(&bus).await.expect("register handlers");

    let state = AppState { bus: bus.clone() };
    let app = Router::new()
        .route("/health", get(health))
        .route("/orders", post(create_order))
        .route("/stats", get(stats))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    info!(%addr, "starting server");

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind listener");
    let server = axum::serve(listener, app);

    let shutdown_bus = bus.clone();
    let shutdown = async move {
        if let Err(err) = tokio::signal::ctrl_c().await {
            error!(error = %err, "failed to listen for ctrl-c");
        }
        info!("shutdown signal received");
        if let Err(err) = shutdown_bus.shutdown().await {
            error!(error = %err, "bus shutdown failed");
        }
    };

    if let Err(err) = server.with_graceful_shutdown(shutdown).await {
        error!(error = %err, "server exited with error");
    }
}

async fn register_handlers(bus: &EventBus) -> Result<(), EventBusError> {
    let _ = bus
        .subscribe_with_policy::<OrderCreated, _, _>(NotificationHandler, SubscriptionPolicy::default().with_priority(20).with_max_retries(2))
        .await?;

    let _ = bus
        .subscribe_with_policy::<OrderCreated, _, _>(
            InventoryProjectionHandler,
            SubscriptionPolicy::default().with_priority(10).with_max_retries(1),
        )
        .await?;

    let _ = bus
        .subscribe_with_policy::<OrderCreated, _, _>(
            AuditLogHandler,
            SyncSubscriptionPolicy::default().with_priority(50).with_dead_letter(false),
        )
        .await?;

    let _ = bus.subscribe_dead_letters(DeadLetterLogger).await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let healthy = state.bus.is_healthy().await;
    (StatusCode::OK, Json(HealthResponse { healthy }))
}

async fn stats(State(state): State<AppState>) -> impl IntoResponse {
    match state.bus.stats().await {
        Ok(stats) => {
            let body = serde_json::json!({
                "total_subscriptions": stats.total_subscriptions,
                "registered_event_types": stats.registered_event_types,
                "in_flight_async": stats.in_flight_async,
                "queue_capacity": stats.queue_capacity,
                "publish_permits_available": stats.publish_permits_available,
                "publish_in_flight": stats.publish_in_flight,
                "shutdown_called": stats.shutdown_called,
            });
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(err) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                error: format!("stats unavailable: {err}"),
            }),
        )
            .into_response(),
    }
}

async fn create_order(State(state): State<AppState>, Json(payload): Json<CreateOrderRequest>) -> impl IntoResponse {
    let event = OrderCreated {
        order_id: payload.order_id.clone(),
        customer_email: payload.customer_email,
        total_cents: payload.total_cents,
    };

    match state.bus.publish(event).await {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(CreateOrderResponse {
                status: "accepted",
                order_id: payload.order_id,
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                error: format!("publish failed: {err}"),
            }),
        )
            .into_response(),
    }
}
