// SPDX-License-Identifier: MIT
//! Retry on sync handler should fail.

use jaeb::HandlerResult;
use summer_jaeb::event_listener;

struct MyEvent;

#[event_listener(retries = 1)]
fn on_event(event: &MyEvent) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
