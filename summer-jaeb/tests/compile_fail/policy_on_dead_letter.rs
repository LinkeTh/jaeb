// SPDX-License-Identifier: MIT
//! Failure policy on dead letter listener should fail.

use jaeb::{DeadLetter, HandlerResult};
use summer_jaeb::event_listener;

#[event_listener(retries = 1)]
fn on_dead_letter(event: &DeadLetter) -> HandlerResult {
    let _ = event;
    Ok(())
}

fn main() {}
