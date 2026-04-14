//! Missing dead-letter event parameter should fail.

use jaeb::{HandlerResult, dead_letter_handler};

#[dead_letter_handler]
fn on_dead_letter() -> HandlerResult {
    Ok(())
}

fn main() {}
