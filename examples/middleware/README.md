# middleware

Shows global and typed pre-dispatch middleware for filtering, logging, or
rejecting events before they reach any handler.

## Run

```bash
cargo run -p middleware
```

## What it demonstrates

| Concept                              | Where                                                                              |
|--------------------------------------|------------------------------------------------------------------------------------|
| `SyncMiddleware` (global)            | `LogAllMiddleware` — sees every event type passing through the bus                 |
| `TypedSyncMiddleware<E>`             | `VipOnlyMiddleware` — only runs for `Order` events; transparent to `Metric` events |
| `MiddlewareDecision::Continue`       | Passes the event to the next middleware or handlers                                |
| `MiddlewareDecision::Reject(reason)` | Short-circuits the pipeline; `publish` returns `Err(MiddlewareRejected)`           |
| `EventBusError::MiddlewareRejected`  | Returned to the caller when a middleware rejects an event                          |
| `bus.add_sync_middleware`            | Registers a global middleware                                                      |
| `bus.add_typed_sync_middleware`      | Registers a typed middleware scoped to one event type                              |

## Expected output

```
[middleware] global: saw event Order
handler: processing order 1
[middleware] global: saw event Order
rejected: order 2 is not VIP
[middleware] global: saw event Metric
handler: recorded metric cpu_usage
```
