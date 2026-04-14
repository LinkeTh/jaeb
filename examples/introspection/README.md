# introspection

Shows the `stats()` and `is_healthy()` APIs for observing bus state at runtime.

## Run

```bash
cargo run -p introspection
```

## What it demonstrates

| Concept                        | Where                                                                                             |
|--------------------------------|---------------------------------------------------------------------------------------------------|
| `bus.stats()`                  | Returns a `BusStats` snapshot: subscription counts, event types, queue capacity, in-flight counts |
| `stats.subscriptions_by_event` | Per-event-type list of listener IDs and names                                                     |
| `handler.name()`               | Optional `&'static str` returned by the handler for identification                                |
| `bus.is_healthy()`             | `true` while the bus is running                                                                   |
| Post-shutdown behaviour        | `is_healthy()` → `false`; `stats()` → `Err(Stopped)`                                              |
