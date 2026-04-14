//! Mutable event reference should fail.

use jaeb::{HandlerResult, handler};

#[derive(Clone)]
struct MyEvent;

#[handler]
async fn on_event(event: &mut MyEvent) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
