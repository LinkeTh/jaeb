//! Wrong return type should fail.

use jaeb::handler;

#[derive(Clone)]
struct MyEvent;

#[handler]
async fn on_event(event: &MyEvent) -> () {
    let _ = event;
}

fn main() {}
