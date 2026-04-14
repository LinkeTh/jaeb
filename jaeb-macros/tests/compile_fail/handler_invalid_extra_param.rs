//! Non-Dep extra parameter should fail.

use jaeb::{HandlerResult, handler};

#[derive(Clone)]
struct MyEvent;

struct Db;

#[handler]
async fn on_event(event: &MyEvent, db: Db) -> HandlerResult {
    let _ = (event, db);
    Ok(())
}

fn main() {}
