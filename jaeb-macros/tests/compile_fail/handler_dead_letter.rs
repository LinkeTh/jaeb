//! `#[handler]` on DeadLetter should fail.

use jaeb::{DeadLetter, HandlerResult, handler};

#[handler]
fn on_dead_letter(event: &DeadLetter) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
