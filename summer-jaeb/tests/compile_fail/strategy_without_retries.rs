//! `retry_strategy` without `retries` should fail.

use jaeb::HandlerResult;
use summer_jaeb::event_listener;

#[derive(Clone)]
struct MyEvent;

#[event_listener(retry_strategy = "exponential", retry_base_ms = 50, retry_max_ms = 5000)]
async fn on_event(event: &MyEvent) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
