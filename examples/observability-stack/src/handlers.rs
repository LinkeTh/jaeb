//! Handler structs for the observability-stack simulation.
//!
//! Each handler has a distinct latency/reliability profile to generate
//! varied metric data in dashboards.
//!
//! Note: handlers no longer need manual span propagation — jaeb automatically
//! instruments async handlers with the publisher's tracing span when the
//! `trace` feature is enabled.

use std::time::Duration;

use jaeb::{DeadLetter, EventHandler, HandlerResult, SyncEventHandler};
use tracing::info;

use crate::events::*;

// ── OrderCreated handlers ──────────────────────────────────────────────────

/// Fast, reliable handler -- simulates writing to a DB.
pub struct OrderPersistHandler;

impl EventHandler<OrderCreated> for OrderPersistHandler {
    async fn handle(&self, event: &OrderCreated) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(5)).await;
        info!(order_id = %event.order_id, amount = event.amount_cents, "order persisted");
        Ok(())
    }

    fn name(&self) -> Option<&'static str> {
        Some("order-persist")
    }
}

/// Slow, reliable handler -- simulates sending a confirmation email.
pub struct OrderEmailHandler;

impl EventHandler<OrderCreated> for OrderEmailHandler {
    async fn handle(&self, event: &OrderCreated) -> HandlerResult {
        // Simulate 80-250 ms email API latency.
        let ms = 80 + (event.amount_cents % 170) as u64;
        tokio::time::sleep(Duration::from_millis(ms)).await;
        info!(order_id = %event.order_id, "confirmation email sent");
        Ok(())
    }

    fn name(&self) -> Option<&'static str> {
        Some("order-email")
    }
}

// ── PaymentProcessed handlers ──────────────────────────────────────────────

/// Flaky handler -- fails on some orders to demonstrate retries and dead letters.
pub struct PaymentLedgerHandler;

impl EventHandler<PaymentProcessed> for PaymentLedgerHandler {
    async fn handle(&self, event: &PaymentProcessed) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(20)).await;
        // Deterministic pseudo-failure keyed on order_id length.
        if !event.success && event.order_id.len() % 3 == 0 {
            return Err("ledger write timeout".into());
        }
        info!(order_id = %event.order_id, success = event.success, "ledger updated");
        Ok(())
    }

    fn name(&self) -> Option<&'static str> {
        Some("payment-ledger")
    }
}

/// Sync handler -- logs payment outcome to an audit table.
pub struct PaymentAuditHandler;

impl SyncEventHandler<PaymentProcessed> for PaymentAuditHandler {
    fn handle(&self, event: &PaymentProcessed) -> HandlerResult {
        info!(order_id = %event.order_id, success = event.success, "payment audited");
        Ok(())
    }

    fn name(&self) -> Option<&'static str> {
        Some("payment-audit")
    }
}

// ── ShipmentDispatched handler ─────────────────────────────────────────────

/// Notifies the warehouse system -- medium latency.
pub struct WarehouseNotifier;

impl EventHandler<ShipmentDispatched> for WarehouseNotifier {
    async fn handle(&self, event: &ShipmentDispatched) -> HandlerResult {
        tokio::time::sleep(Duration::from_millis(40)).await;
        info!(order_id = %event.order_id, carrier = event.carrier, "warehouse notified");
        Ok(())
    }

    fn name(&self) -> Option<&'static str> {
        Some("warehouse-notify")
    }
}

// ── FraudCheckFailed handler -- always fails ───────────────────────────────

/// Always-failing handler to demonstrate the dead-letter pipeline.
pub struct FraudAlertHandler;

impl EventHandler<FraudCheckFailed> for FraudAlertHandler {
    async fn handle(&self, event: &FraudCheckFailed) -> HandlerResult {
        Err(format!("alert system unreachable for order {} (reason: {})", event.order_id, event.reason).into())
    }

    fn name(&self) -> Option<&'static str> {
        Some("fraud-alert")
    }
}

// ── Dead-letter sink ───────────────────────────────────────────────────────

/// Receives dead letters and records a custom Prometheus counter so dashboards
/// can track DLQ depth.
pub struct DeadLetterSink;

impl SyncEventHandler<DeadLetter> for DeadLetterSink {
    fn handle(&self, dl: &DeadLetter) -> HandlerResult {
        // Increment a custom Prometheus counter so dashboards track DLQ depth.
        metrics::counter!(
            "jaeb.dead_letter.total",
            "event" => dl.event_name,
            "handler" => dl.handler_name.unwrap_or("unknown"),
        )
        .increment(1);

        tracing::warn!(
            event = dl.event_name,
            handler = dl.handler_name,
            attempts = dl.attempts,
            error = %dl.error,
            "dead letter received"
        );
        Ok(())
    }

    fn name(&self) -> Option<&'static str> {
        Some("dead-letter-sink")
    }
}
