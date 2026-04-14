# sync-handler

Shows `SyncEventHandler`: handlers execute inline during `publish` in a
serialized FIFO lane, and `publish` only returns after all of them finish.

## Run

```bash
cargo run -p sync-handler
```

## What it demonstrates

| Concept                             | Where                                                                             |
|-------------------------------------|-----------------------------------------------------------------------------------|
| `SyncEventHandler<E>` trait         | `fn handle(&self, event: &E, bus: &EventBus) -> HandlerResult` — no `async`, no cloning overhead |
| Serialized FIFO execution           | `FirstHandler` always runs before `SecondHandler` (subscription order)            |
| `publish` blocks until completion   | The caller can rely on all sync handlers having run by the time `publish` returns |
| `Clone` still required on the event | `publish` needs `E: Clone` even for sync-only subscriptions                       |

## Expected output

```
[seq=0] first handler: order 42 placed
[seq=1] second handler: order 42 placed
total invocations: 2
```
