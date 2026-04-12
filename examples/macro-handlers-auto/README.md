# macro-handlers-auto

Demonstrates the `#[handler]` macro combined with `register_handlers!(bus)` for
**zero-boilerplate auto-discovery** — no handler names need to be listed manually.

## Run

```bash
cargo run -p macro-handlers-auto
```

## What it demonstrates

| Concept | Where |
|---|---|
| `#[handler]` macro | Annotates a free function; generates the handler struct and inventory registrar |
| `register_handlers!(bus)` | Discovers **all** `#[handler]` functions in the crate via `inventory` and subscribes them in one call |
| No explicit listing | Unlike `macro-handlers`, every handler is picked up automatically |

## Contrast with `macro-handlers`

`register_handlers!(bus)` with no extra arguments subscribes every handler
found by `inventory`. Use `register_handlers!(bus, fn1, fn2)` (see the
`macro-handlers` example) when you need explicit control over which handlers
are registered.
