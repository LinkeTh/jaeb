//! `retry_base_ms` without `retry_strategy` should fail.

use jaeb::HandlerResult;
use summer_jaeb::event_listener;

#[derive(Clone)]
struct MyEvent;

#[event_listener(retries = 3, retry_base_ms = 100)]
async fn on_event(event: &MyEvent) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
