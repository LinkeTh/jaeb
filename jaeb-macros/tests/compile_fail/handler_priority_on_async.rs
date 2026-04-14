//! Priority on async handler should fail.

use jaeb::{HandlerResult, handler};

#[derive(Clone)]
struct MyEvent;

#[handler(priority = 1)]
async fn on_event(event: &MyEvent) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
