# closure-handlers

Register closures directly instead of defining handler structs. Useful for
lightweight, one-off subscriptions.

## Run

```bash
cargo run -p closure-handlers
```

## What it demonstrates

| Concept | Where |
|---|---|
| Sync closure | `\|event: &Clicked\| -> HandlerResult { ... }` — receives `&E` |
| Async closure | `\|event: Clicked\| async move { ... }` — receives `E` by value (a clone) |
| Both on the same event type | Both closures subscribed to `Clicked`; both fire on a single publish |

> **Note:** Async closures receive the event **by value** because events are
> cloned per async invocation. Sync closures receive a shared reference.
