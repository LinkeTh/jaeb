//! Async dead letter listener should fail.

use jaeb::{DeadLetter, HandlerResult};
use summer_jaeb::event_listener;

#[event_listener]
async fn on_dead_letter(event: &DeadLetter) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
