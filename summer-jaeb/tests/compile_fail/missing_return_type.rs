// SPDX-License-Identifier: MIT
//! Missing return type should fail.

use summer_jaeb::event_listener;

#[derive(Clone)]
struct MyEvent;

#[event_listener]
async fn on_event(event: &MyEvent) {
    let _ = event;
}

fn main() {}
