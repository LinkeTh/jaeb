//! Event types for the observability-stack simulation.
//!
//! With jaeb's built-in span propagation (`trace` feature), events no longer
//! need to carry tracing context — the bus captures `Span::current()` at
//! publish time and instruments spawned async handler futures automatically.

// ── Events ─────────────────────────────────────────────────────────────────

/// Emitted when a new order is placed by a (simulated) customer.
#[derive(Clone, Debug)]
pub struct OrderCreated {
    pub order_id: String,
    pub amount_cents: u32,
}

/// Emitted after a payment gateway call (may succeed or fail in simulation).
#[derive(Clone, Debug)]
pub struct PaymentProcessed {
    pub order_id: String,
    pub success: bool,
}

/// Emitted when inventory is decremented and a shipment label is created.
#[derive(Clone, Debug)]
pub struct ShipmentDispatched {
    pub order_id: String,
    pub carrier: &'static str,
}

/// A critical event that always fails -- demonstrates dead-letter pipeline.
#[derive(Clone, Debug)]
pub struct FraudCheckFailed {
    pub order_id: String,
    pub reason: String,
}
