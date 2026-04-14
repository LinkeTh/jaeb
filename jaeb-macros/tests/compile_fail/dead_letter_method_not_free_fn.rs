//! Dead-letter handler on a method should fail.

use jaeb::{DeadLetter, HandlerResult, dead_letter_handler};

struct Receiver;

impl Receiver {
    #[dead_letter_handler]
    fn on_dead_letter(&self, event: &DeadLetter) -> HandlerResult {
        let _ = event;
        Ok(())
    }
}

fn main() {}
