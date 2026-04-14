//! Async dead-letter handler should fail.

use jaeb::{DeadLetter, HandlerResult, dead_letter_handler};

#[dead_letter_handler]
async fn on_dead_letter(event: &DeadLetter) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
