//! Missing explicit return type on dead-letter handler should fail.

use jaeb::{DeadLetter, dead_letter_handler};

#[dead_letter_handler]
fn on_dead_letter(event: &DeadLetter) {
    let _ = event;
}

fn main() {}
