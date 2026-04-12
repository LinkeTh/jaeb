# graceful-shutdown

Covers all three shutdown outcomes: clean drain, timeout abort, and
`Stopped` errors on post-shutdown publishes.

## Run

```bash
cargo run -p graceful-shutdown
```

## What it demonstrates

| Concept | Where |
|---|---|
| `shutdown_timeout` builder option | Configures a deadline after which remaining async tasks are aborted |
| Scenario A — clean drain | `QuickHandler` (50 ms) finishes before the 2 s timeout → `Ok(())` |
| Scenario B — timeout | `VerySlowHandler` (5 s) exceeds the 200 ms timeout → `Err(ShutdownTimeout)` |
| `EventBusError::ShutdownTimeout` | Returned when the drain deadline is exceeded |
| `EventBusError::Stopped` | Returned by `publish` after `shutdown` completes |

## Expected output

```
quick job completed
scenario A: drained cleanly
scenario B: timed out (expected)
post-shutdown publish: Stopped (expected)
```
