use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use jaeb::{DeadLetter, Dep, Deps, EventBus, HandlerResult, SubscriptionPolicy, dead_letter_handler, handler};
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

struct DbPool;

impl DbPool {
    fn is_connected(&self) -> bool {
        true
    }

    fn update_inventory(&self, order_id: &str) {
        info!(order_id, "db: inventory row updated");
    }

    fn record_audit(&self, order_id: &str, total_cents: u64) {
        info!(order_id, total_cents, "db: audit row inserted");
    }
}

struct Mailer;

impl Mailer {
    fn is_available(&self) -> bool {
        true
    }

    fn send(&self, to: &str, subject: &str) {
        info!(to, subject, "mailer: email queued");
    }
}

#[derive(Clone)]
struct AppState {
    bus: EventBus,
    db: Arc<DbPool>,
    mailer: Arc<Mailer>,
}

#[handler(retries = 2, dead_letter = true, priority = 20, name = "notification")]
async fn notify_customer(event: &OrderCreated, Dep(mailer): Dep<Arc<Mailer>>) -> HandlerResult {
    tokio::time::sleep(Duration::from_millis(10)).await;
    mailer.send(&event.customer_email, &format!("Order {} confirmed", event.order_id));
    info!(
        order_id = %event.order_id,
        customer_email = %event.customer_email,
        "notification sent"
    );
    Ok(())
}

#[handler(retries = 1, dead_letter = true, priority = 10, name = "inventory")]
async fn project_inventory(event: &OrderCreated, db: Dep<Arc<DbPool>>) -> HandlerResult {
    tokio::time::sleep(Duration::from_millis(5)).await;
    db.0.update_inventory(&event.order_id);
    info!(order_id = %event.order_id, "inventory projection updated");
    Ok(())
}

#[handler(priority = 50, dead_letter = false, name = "audit")]
fn append_audit(event: &OrderCreated, Dep(db): Dep<Arc<DbPool>>) -> HandlerResult {
    db.record_audit(&event.order_id, event.total_cents);
    info!(
        order_id = %event.order_id,
        total_cents = event.total_cents,
        "audit log appended"
    );
    Ok(())
}

#[dead_letter_handler]
fn log_dead_letter(event: &DeadLetter) -> HandlerResult {
    warn!(
        dead_letter_event = event.event_name,
        subscription_id = event.subscription_id.as_u64(),
        attempts = event.attempts,
        error = %event.error,
        "dead letter received"
    );
    Ok(())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,jaeb=trace,axum_integration_macros=debug".into()),
        )
        .init();

    let db = Arc::new(DbPool);
    let mailer = Arc::new(Mailer);

    let bus = EventBus::builder()
        .buffer_size(256)
        .max_concurrent_async(64)
        .default_subscription_policy(SubscriptionPolicy::default().with_priority(0))
        .handler(notify_customer)
        .handler(project_inventory)
        .handler(append_audit)
        .dead_letter(log_dead_letter)
        .deps(Deps::new().insert(db.clone()).insert(mailer.clone()))
        .build()
        .await
        .expect("valid bus config");

    let state = AppState {
        bus: bus.clone(),
        db,
        mailer,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/orders", post(create_order))
        .route("/stats", get(stats))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3001));
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

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let bus_healthy = state.bus.is_healthy().await;
    let db_connected = state.db.is_connected();
    let mailer_available = state.mailer.is_available();
    let healthy = bus_healthy && db_connected && mailer_available;
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
