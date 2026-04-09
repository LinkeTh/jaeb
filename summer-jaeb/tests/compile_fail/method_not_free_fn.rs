// SPDX-License-Identifier: MIT
//! `#[event_listener]` on a method should fail.

use jaeb::HandlerResult;
use summer_jaeb::event_listener;

#[derive(Clone)]
struct MyEvent;

struct MyStruct;

impl MyStruct {
    #[event_listener]
    async fn on_event(self, event: &MyEvent) -> HandlerResult {
        Ok(())
    }
}

fn main() {}
