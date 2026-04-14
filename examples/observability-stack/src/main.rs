//! Observability stack example -- full integration of jaeb metrics and tracing
//! with Prometheus, Loki, Tempo, and Grafana.
//!
//! Span propagation is handled automatically by jaeb's `trace` feature --
//! publishing inside an active tracing span causes all async handlers to
//! inherit that span context without any event payload changes.

mod events;
mod handlers;
mod sim;
mod telemetry;

use std::time::Duration;

use jaeb::{AsyncSubscriptionPolicy, EventBus, RetryStrategy};
use tokio::sync::watch;
use tracing::info;

use handlers::*;

#[tokio::main]
async fn main() {
    // ── Read config from env (with sane defaults for local dev) ────────────
    let loki_url = std::env::var("LOKI_URL").unwrap_or_else(|_| "http://localhost:3100".into());
    let otlp_url = std::env::var("OTLP_ENDPOINT").unwrap_or_else(|_| "http://localhost:4317".into());
    let prom_port: u16 = std::env::var("PROMETHEUS_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    let tick_ms: u64 = std::env::var("TICK_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(500);

    // ── Telemetry ──────────────────────────────────────────────────────────
    telemetry::init_prometheus(prom_port);
    let tel = telemetry::init_tracing(&loki_url, &otlp_url).await;

    info!("observability-stack starting");

    // ── Event bus ──────────────────────────────────────────────────────────
    let bus = EventBus::builder()
        .shutdown_timeout(Duration::from_secs(5))
        .build()
        .await
        .expect("bus config valid");

    // ── Subscriptions ──────────────────────────────────────────────────────

    // OrderCreated handlers (default policy -- reliable).
    let _ = bus.subscribe(OrderPersistHandler).await.expect("subscribe failed");
    let _ = bus.subscribe(OrderEmailHandler).await.expect("subscribe failed");

    // PaymentProcessed: flaky ledger handler with retries + dead-letter.
    let retry_policy = AsyncSubscriptionPolicy::default()
        .with_max_retries(2)
        .with_retry_strategy(RetryStrategy::ExponentialWithJitter {
            base: Duration::from_millis(50),
            max: Duration::from_millis(500),
        })
        .with_dead_letter(true);
    let _ = bus
        .subscribe_with_policy(PaymentLedgerHandler, retry_policy)
        .await
        .expect("subscribe failed");

    // PaymentProcessed: reliable sync audit handler.
    let _ = bus.subscribe(PaymentAuditHandler).await.expect("subscribe failed");

    // ShipmentDispatched: reliable warehouse notifier.
    let _ = bus.subscribe(WarehouseNotifier).await.expect("subscribe failed");

    // FraudCheckFailed: always-failing handler with dead-letter.
    let fraud_policy = AsyncSubscriptionPolicy::default().with_max_retries(1).with_dead_letter(true);
    let _ = bus
        .subscribe_with_policy(FraudAlertHandler, fraud_policy)
        .await
        .expect("subscribe failed");

    // Dead-letter sink.
    let _ = bus.subscribe_dead_letters(DeadLetterSink).await.expect("dead-letter subscribe failed");

    // ── Shutdown signal ────────────────────────────────────────────────────
    let (stop_tx, stop_rx) = watch::channel(false);

    tokio::spawn(async move {
        #[cfg(unix)]
        {
            // Listen for both SIGTERM (Docker stop) and Ctrl+C (local dev).
            let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler failed");

            tokio::select! {
                _ = sigterm.recv() => {
                    info!("SIGTERM received, stopping simulation");
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("Ctrl+C received, stopping simulation");
                }
            }
        }

        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await.expect("Ctrl+C handler failed");
            info!("Ctrl+C received, stopping simulation");
        }

        let _ = stop_tx.send(true);
    });

    // ── Main simulation loop ───────────────────────────────────────────────
    sim::run_loop(bus.clone(), stop_rx, tick_ms).await;

    // ── Graceful shutdown ──────────────────────────────────────────────────
    info!("shutting down event bus");
    match bus.shutdown().await {
        Ok(()) => info!("bus drained cleanly"),
        Err(e) => tracing::warn!(error = %e, "bus shutdown with error"),
    }

    telemetry::shutdown_telemetry(tel);
    info!("shutdown complete");
}
