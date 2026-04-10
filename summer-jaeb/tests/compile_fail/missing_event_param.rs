//! Missing event parameter should fail.

use jaeb::HandlerResult;
use summer_jaeb::event_listener;

#[event_listener]
async fn on_event() -> HandlerResult {
    Ok(())
}

fn main() {}
