//! Retry strategy attributes without retries should fail.

use jaeb::{HandlerResult, handler};

#[derive(Clone)]
struct MyEvent;

#[handler(retry_strategy = "fixed", retry_base_ms = 100)]
async fn on_event(event: &MyEvent) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
