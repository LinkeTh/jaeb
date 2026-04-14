//! Wrong first parameter for dead-letter handler should fail.

use jaeb::{HandlerResult, dead_letter_handler};

#[derive(Clone)]
struct MyEvent;

#[dead_letter_handler]
fn on_dead_letter(event: &MyEvent) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
