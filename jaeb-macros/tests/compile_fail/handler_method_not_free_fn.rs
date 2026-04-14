//! Handler on a method should fail.

use jaeb::{HandlerResult, handler};

#[derive(Clone)]
struct MyEvent;

struct Receiver;

impl Receiver {
    #[handler]
    async fn on_event(&self, event: &MyEvent) -> HandlerResult {
        let _ = event;
        Ok(())
    }
}

fn main() {}
