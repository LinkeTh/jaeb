//! Missing event parameter should fail.

use jaeb::{HandlerResult, handler};

#[handler]
async fn on_event() -> HandlerResult {
    Ok(())
}

fn main() {}
