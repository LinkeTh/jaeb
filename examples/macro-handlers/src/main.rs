//! Demonstrates `#[handler]` and `#[dead_letter_handler]` with the builder
//! pattern, including handlers that receive injected `Dep<T>` dependencies.
//!
//! `#[handler]` generates a struct that implements `HandlerDescriptor`,
//! registered via `EventBusBuilder::handler`.
//!
//! `#[dead_letter_handler]` generates a struct that implements
//! `DeadLetterDescriptor`, registered via
//! `EventBusBuilder::dead_letter`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jaeb::{DeadLetter, Dep, Deps, EventBus, HandlerResult, dead_letter_handler, handler};

#[derive(Clone, Debug)]
struct Payment {
    id: u32,
}

/// A shared audit log passed as a dependency to handlers.
#[derive(Clone)]
struct AuditLog(Arc<AtomicUsize>);

/// Async handler with a `Dep<T>` parameter.  The `AuditLog` is resolved from
/// the `Deps` container at bus build time and cloned into each invocation.
#[handler]
async fn process_payment(event: &Payment, Dep(log): Dep<AuditLog>) -> HandlerResult {
    println!("processing payment {}", event.id);
    log.0.fetch_add(1, Ordering::SeqCst);
    Err(format!("simulated failure for payment {}", event.id).into())
}

/// A synchronous dead-letter handler that also receives a dep.
#[dead_letter_handler]
fn on_dead_letter(event: &DeadLetter, Dep(log): Dep<AuditLog>) -> HandlerResult {
    println!(
        "dead-letter: event={}, handler={:?}, attempts={}, error={}",
        event.event_name, event.handler_name, event.attempts, event.error
    );
    log.0.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), jaeb::EventBusError> {
    let audit = AuditLog(Arc::new(AtomicUsize::new(0)));

    let bus = EventBus::builder()
        .buffer_size(64)
        .handler(process_payment)
        .dead_letter(on_dead_letter)
        .deps(Deps::new().insert(audit.clone()))
        .build()
        .await?;

    bus.publish(Payment { id: 7 }).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;

    bus.shutdown().await?;

    println!("audit log entries: {}", audit.0.load(Ordering::SeqCst));
    Ok(())
}
