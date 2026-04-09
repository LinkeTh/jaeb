// SPDX-License-Identifier: MIT
//! Mutable event reference should fail.

use jaeb::HandlerResult;
use summer_jaeb::event_listener;

#[derive(Clone)]
struct MyEvent;

#[event_listener]
async fn on_event(event: &mut MyEvent) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
