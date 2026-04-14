//! retry_base_ms without retry_strategy should fail.

use jaeb::{HandlerResult, handler};

#[derive(Clone)]
struct MyEvent;

#[handler(retries = 1, retry_base_ms = 100)]
async fn on_event(event: &MyEvent) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
