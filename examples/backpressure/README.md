# backpressure

Demonstrates `try_publish` (non-blocking) vs `publish` (blocking) and how a small
buffer creates back-pressure when handlers can't keep up.

## Run

```bash
cargo run -p backpressure
```

## What it demonstrates

| Concept | Where |
|---|---|
| `try_publish` | Spawns dispatch immediately; returns `Err(ChannelFull)` when no permit is free |
| `EventBusError::ChannelFull` | Returned on the third `try_publish` while two slow handlers are in flight |
| `publish` (blocking) | Waits for a permit to become available before dispatching |
| Small `buffer_size` | Set to 2 so back-pressure is visible with only a few events |

## Expected output

```
two tasks queued
try_publish 3: ChannelFull (expected)
blocking publish waiting for a free slot...
processed task 1
blocking publish succeeded
processed task 2
processed task 3
```
