# basic-pubsub

The simplest possible jaeb example: one event, one async handler, one publish.
A good starting point for understanding the core API.

## Run

```bash
cargo run -p basic-pubsub
```

## What it demonstrates

| Concept                                              | Where                                                      |
|------------------------------------------------------|------------------------------------------------------------|
| `EventBus::builder().build().await` | Creates a bus with default runtime settings                |
| `bus.subscribe`                                      | Registers an `EventHandler` implementation                 |
| `bus.publish`                                        | Dispatches an event; returns after async work is spawned   |
| `bus.shutdown`                                       | Drains in-flight tasks and stops the bus                   |
