//! bus parameter on dead-letter handler should fail.

use jaeb::{DeadLetter, EventBus, HandlerResult, dead_letter_handler};

#[dead_letter_handler]
fn on_dead_letter(event: &DeadLetter, bus: &EventBus) -> HandlerResult {
    let _ = (event, bus);
    Ok(())
}

fn main() {}
