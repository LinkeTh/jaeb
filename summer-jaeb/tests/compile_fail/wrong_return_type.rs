//! Wrong return type should fail.

use summer_jaeb::event_listener;

#[derive(Clone)]
struct MyEvent;

#[event_listener]
async fn on_event(event: &MyEvent) -> Result<(), String> {
    let _ = event;
    Ok(())
}

fn main() {}
