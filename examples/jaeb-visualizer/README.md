# jaeb-visualizer

An interactive terminal UI (TUI) that runs a live jaeb simulation and renders
real-time metrics — publish throughput, handler latency, error rates, and
dead-letter counts — in a dashboard built with [ratatui](https://ratatui.rs).

## Run

```bash
cargo run -p jaeb-visualizer
```

Requires a terminal with at least 80×24 characters.

## Usage

| Key            | Action                                                     |
|----------------|------------------------------------------------------------|
| `Enter`        | Start the simulation from the config screen                |
| `Tab` / arrows | Navigate config fields                                     |
| `r`            | Re-run simulation with the same config (after it finishes) |
| `c`            | Return to the config screen to change parameters           |
| `q`            | Quit                                                       |

## Screens

**Config screen** — set bus parameters (buffer size, concurrency limit) and
listener properties (count, failure rate, retry count) before starting.

**Dashboard** — live panels showing:

- Publish rate and cumulative volume
- Per-handler invocation counts and error rates
- Dead-letter counter
- In-flight async task gauge

## What it demonstrates

| Concept                         | Where                                                                |
|---------------------------------|----------------------------------------------------------------------|
| Real-time `bus.stats()` polling | `metrics` module samples the bus on every tick                       |
| Configurable simulation         | `SimConfig` drives listener count, failure rates, publish interval   |
| ratatui integration             | `ui/` module renders config and dashboard frames at 100 ms tick rate |
| Graceful shutdown on quit       | `sim_handle.stop()` triggers `bus.shutdown()` before exiting         |
