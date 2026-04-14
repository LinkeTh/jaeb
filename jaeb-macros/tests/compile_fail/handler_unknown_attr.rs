//! Unknown attribute should fail.

use jaeb::{HandlerResult, handler};

#[derive(Clone)]
struct MyEvent;

#[handler(unknown = 42)]
async fn on_event(event: &MyEvent) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
