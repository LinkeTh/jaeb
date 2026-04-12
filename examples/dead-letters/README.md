# dead-letters

Demonstrates the full dead-letter pipeline: a handler that always fails exhausts
its retries and the event is routed to a `DeadLetter` sink.

## Run

```bash
cargo run -p dead-letters
```

## What it demonstrates

| Concept | Where |
|---|---|
| `with_dead_letter(true)` | Enables dead-letter routing on terminal failure |
| `subscribe_dead_letters` | Registers a `SyncEventHandler<DeadLetter>` sink; forces `dead_letter=false` to prevent recursion |
| `DeadLetter` fields | `event_name`, `handler_name`, `subscription_id`, `attempts`, `error` |
| Payload downcast | `dl.event.downcast_ref::<Payment>()` recovers the original event |
| Fixed retry strategy | 2 retries at 50 ms intervals before terminal failure |

## Flow

```
publish(Payment { id: 99 })
  → OnPayment: fails (attempt 1)
  → OnPayment: fails (attempt 2 — retry)
  → OnPayment: fails (attempt 3 — retry)
  → dead letter emitted
  → DeadLetterSink: prints original payload
```
