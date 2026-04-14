# panic-safety

Verifies that a panicking handler does not crash the bus or affect other
unrelated handlers.

## Run

```bash
cargo run -p panic-safety
```

## What it demonstrates

| Concept                  | Where                                                                                       |
|--------------------------|---------------------------------------------------------------------------------------------|
| Panic isolation          | `PanicHandler` calls `panic!()` — the bus catches it internally                             |
| Bus continues operating  | After the panic, `SafeCounter` handles `Safe` events normally                               |
| Cross-event independence | `Boom` and `Safe` are separate event types; a failure in one lane does not affect the other |

## Expected output

```
safe handler invoked
bus survived the panic, safe count = 1
```
