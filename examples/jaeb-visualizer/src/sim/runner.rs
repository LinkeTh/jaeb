use std::sync::{Arc, Mutex};
use std::time::Duration;

use jaeb::{EventBus, FailurePolicy, RetryStrategy};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::config::types::*;
use crate::metrics::VisualizationState;
use crate::metrics::collector::{run_collector, run_sampler};
use crate::sim::events::SimEvent;
use crate::sim::handlers::*;
use crate::sim::publisher::run_publish_loop;

pub struct SimHandle {
    bus: EventBus,
    _sim_task: JoinHandle<()>,
    _collector_task: JoinHandle<()>,
    _sampler_task: JoinHandle<()>,
}

impl SimHandle {
    pub fn request_shutdown(&self) {
        let bus = self.bus.clone();
        tokio::spawn(async move {
            let _ = bus.shutdown().await;
        });
    }
}

/// Build the bus, register handlers, start publishing, and return handles.
pub fn launch_simulation(config: SimConfig, viz: Arc<Mutex<VisualizationState>>) -> SimHandle {
    let (tx, rx) = mpsc::unbounded_channel::<SimEvent>();

    // Build the event bus
    let mut builder = EventBus::builder().buffer_size(config.bus.buffer_size);

    if config.bus.handler_timeout_ms > 0 {
        builder = builder.handler_timeout(Duration::from_millis(config.bus.handler_timeout_ms));
    }
    if config.bus.max_concurrent_async > 0 {
        builder = builder.max_concurrent_async(config.bus.max_concurrent_async);
    }
    builder = builder.default_failure_policy(FailurePolicy::default());

    let shutdown_timeout = Duration::from_millis(config.bus.shutdown_timeout_ms);
    builder = builder.shutdown_timeout(shutdown_timeout);

    let bus = builder.build().expect("failed to build EventBus");

    let bus_clone = bus.clone();
    let tx_clone = tx.clone();
    let publish_config = config.publish.clone();
    let listeners = config.listeners.clone();

    // Spawn simulation task: registers handlers then runs publish loop
    let sim_task = tokio::spawn(async move {
        // Register handlers
        for listener_cfg in &listeners {
            let policy = build_failure_policy(listener_cfg);
            match (listener_cfg.event_type_idx, &listener_cfg.mode) {
                (0, HandlerMode::Async) => {
                    let handler = MockAsyncHandlerA {
                        name: listener_cfg.name.clone(),
                        processing_ms: listener_cfg.processing_ms,
                        failure_rate: listener_cfg.failure_rate,
                        tx: tx_clone.clone(),
                    };
                    let _ = bus_clone.subscribe_with_policy::<SimEventA, _, _>(handler, policy).await;
                }
                (0, HandlerMode::Sync) => {
                    let handler = MockSyncHandlerA {
                        name: listener_cfg.name.clone(),
                        processing_ms: listener_cfg.processing_ms,
                        failure_rate: listener_cfg.failure_rate,
                        tx: tx_clone.clone(),
                    };
                    let no_retry = jaeb::NoRetryPolicy {
                        dead_letter: listener_cfg.dead_letter,
                    };
                    let _ = bus_clone.subscribe_with_policy::<SimEventA, _, _>(handler, no_retry).await;
                }
                (1, HandlerMode::Async) => {
                    let handler = MockAsyncHandlerB {
                        name: listener_cfg.name.clone(),
                        processing_ms: listener_cfg.processing_ms,
                        failure_rate: listener_cfg.failure_rate,
                        tx: tx_clone.clone(),
                    };
                    let _ = bus_clone.subscribe_with_policy::<SimEventB, _, _>(handler, policy).await;
                }
                (1, HandlerMode::Sync) => {
                    let handler = MockSyncHandlerB {
                        name: listener_cfg.name.clone(),
                        processing_ms: listener_cfg.processing_ms,
                        failure_rate: listener_cfg.failure_rate,
                        tx: tx_clone.clone(),
                    };
                    let no_retry = jaeb::NoRetryPolicy {
                        dead_letter: listener_cfg.dead_letter,
                    };
                    let _ = bus_clone.subscribe_with_policy::<SimEventB, _, _>(handler, no_retry).await;
                }
                _ => {}
            }
        }

        // Register dead letter collector
        let dl_collector = DeadLetterCollector { tx: tx_clone.clone() };
        let _ = bus_clone.subscribe_dead_letters(dl_collector).await;

        // Run publish loop
        run_publish_loop(bus_clone.clone(), publish_config, tx_clone).await;

        // Shutdown bus after publishing is done
        let _ = bus_clone.shutdown().await;
    });

    // Collector task
    let collector_task = tokio::spawn(run_collector(rx, Arc::clone(&viz), config.event_types.clone()));

    // Sampler task
    let sampler_task = tokio::spawn(run_sampler(Arc::clone(&viz), bus.clone()));

    SimHandle {
        bus,
        _sim_task: sim_task,
        _collector_task: collector_task,
        _sampler_task: sampler_task,
    }
}

fn build_failure_policy(cfg: &ListenerConfig) -> FailurePolicy {
    let mut policy = FailurePolicy::default()
        .with_max_retries(cfg.max_retries as usize)
        .with_dead_letter(cfg.dead_letter);

    match &cfg.retry_strategy {
        RetryStrategyChoice::None => {}
        RetryStrategyChoice::Fixed { delay_ms } => {
            policy = policy.with_retry_strategy(RetryStrategy::Fixed(Duration::from_millis(*delay_ms)));
        }
        RetryStrategyChoice::Exponential { base_ms, max_ms } => {
            policy = policy.with_retry_strategy(RetryStrategy::Exponential {
                base: Duration::from_millis(*base_ms),
                max: Duration::from_millis(*max_ms),
            });
        }
        RetryStrategyChoice::ExponentialWithJitter { base_ms, max_ms } => {
            policy = policy.with_retry_strategy(RetryStrategy::ExponentialWithJitter {
                base: Duration::from_millis(*base_ms),
                max: Duration::from_millis(*max_ms),
            });
        }
    }

    policy
}
