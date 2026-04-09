// SPDX-License-Identifier: MIT
//! Unknown attribute should fail.

use jaeb::HandlerResult;
use summer_jaeb::event_listener;

#[derive(Clone)]
struct MyEvent;

#[event_listener(unknown = 42)]
async fn on_event(event: &MyEvent) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
