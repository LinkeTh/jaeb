# basic-pubsub

The simplest possible jaeb example: one event, one async handler, one publish.
A good starting point for understanding the core API.

## Run

```bash
cargo run -p basic-pubsub
```

## What it demonstrates

| Concept | Where |
|---|---|
| `EventBus::builder().buffer_size(...).build().await` | Creates a bus with a fixed-size publish buffer |
| `bus.subscribe` | Registers an `EventHandler` implementation |
| `bus.publish` | Dispatches an event; returns after async tasks are spawned |
| `bus.shutdown` | Drains in-flight tasks and stops the bus |
