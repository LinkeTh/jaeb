//! Priority on async listener should fail.

use jaeb::HandlerResult;
use summer_jaeb::event_listener;

struct MyEvent;

#[event_listener(priority = 1)]
async fn on_event(event: &MyEvent) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
