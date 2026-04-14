use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use jaeb::{
    AsyncSubscriptionPolicy, DeadLetter, EventBus, EventBusError, EventHandler, HandlerResult, SubscriptionDefaults, SyncEventHandler,
    SyncSubscriptionPolicy,
};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Domain events
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct OrderCreated {
    order_id: String,
    customer_email: String,
    total_cents: u64,
}

// ---------------------------------------------------------------------------
// HTTP request / response types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Application services
//
// In a real application these would hold connection pools, SMTP clients, etc.
// Here they are stubs that log to illustrate the dependency-injection pattern:
// construct once, share via Arc, inject into whichever handlers need them.
// ---------------------------------------------------------------------------

struct DbPool;

impl DbPool {
    fn is_connected(&self) -> bool {
        true // stub: would ping the real pool
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
        true // stub: would check SMTP connectivity
    }

    fn send(&self, to: &str, subject: &str) {
        info!(to, subject, "mailer: email queued");
    }
}

// ---------------------------------------------------------------------------
// Shared axum state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    bus: EventBus,
    db: Arc<DbPool>,
    mailer: Arc<Mailer>,
}

// ---------------------------------------------------------------------------
// Event handlers
//
// Each handler struct holds only the dependencies it actually needs.
// Dependencies are injected at startup (in `register_handlers`) and stored
// as Arc fields — zero overhead at dispatch time.
// ---------------------------------------------------------------------------

struct NotificationHandler {
    mailer: Arc<Mailer>,
}

impl EventHandler<OrderCreated> for NotificationHandler {
    async fn handle(&self, event: &OrderCreated, _bus: &EventBus) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(10)).await;
        self.mailer.send(&event.customer_email, &format!("Order {} confirmed", event.order_id));
        info!(
            order_id = %event.order_id,
            customer_email = %event.customer_email,
            "notification sent"
        );
        Ok(())
    }
}

struct InventoryProjectionHandler {
    db: Arc<DbPool>,
}

impl EventHandler<OrderCreated> for InventoryProjectionHandler {
    async fn handle(&self, event: &OrderCreated, _bus: &EventBus) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(5)).await;
        self.db.update_inventory(&event.order_id);
        info!(order_id = %event.order_id, "inventory projection updated");
        Ok(())
    }
}

struct AuditLogHandler {
    db: Arc<DbPool>,
}

impl SyncEventHandler<OrderCreated> for AuditLogHandler {
    fn handle(&self, event: &OrderCreated, _bus: &EventBus) -> HandlerResult {
        self.db.record_audit(&event.order_id, event.total_cents);
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
    fn handle(&self, dl: &DeadLetter, _bus: &EventBus) -> HandlerResult {
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

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,jaeb=trace,axum_integration=debug".into()))
        .init();

    let bus = EventBus::builder()
        .max_concurrent_async(64)
        .default_subscription_policies(SubscriptionDefaults::default())
        .build()
        .await
        .expect("valid bus config");

    // Construct shared services once; clone the Arc into each handler that
    // needs them at registration time.
    let db = Arc::new(DbPool);
    let mailer = Arc::new(Mailer);

    register_handlers(&bus, &db, &mailer).await.expect("register handlers");

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

// ---------------------------------------------------------------------------
// Handler registration
//
// Each handler is constructed with its dependencies here, at startup.
// The bus receives a fully-formed handler object; at dispatch time it simply
// calls `handler.handle(event)` with no extra allocation.
// ---------------------------------------------------------------------------

async fn register_handlers(bus: &EventBus, db: &Arc<DbPool>, mailer: &Arc<Mailer>) -> Result<(), EventBusError> {
    let _ = bus
        .subscribe_with_policy::<OrderCreated, _, _>(
            NotificationHandler { mailer: mailer.clone() },
            AsyncSubscriptionPolicy::default().with_max_retries(2),
        )
        .await?;

    let _ = bus
        .subscribe_with_policy::<OrderCreated, _, _>(
            InventoryProjectionHandler { db: db.clone() },
            AsyncSubscriptionPolicy::default().with_max_retries(1),
        )
        .await?;

    let _ = bus
        .subscribe_with_policy::<OrderCreated, _, _>(
            AuditLogHandler { db: db.clone() },
            SyncSubscriptionPolicy::default().with_priority(50).with_dead_letter(false),
        )
        .await?;

    let _ = bus.subscribe_dead_letters(DeadLetterLogger).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Axum route handlers
// ---------------------------------------------------------------------------

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let bus_healthy = state.bus.is_healthy();
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
                "dispatches_in_flight": stats.dispatches_in_flight,
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
