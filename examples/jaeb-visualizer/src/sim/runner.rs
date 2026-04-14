use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jaeb::{AsyncSubscriptionPolicy, EventBus, RetryStrategy, SubscriptionDefaults, SyncSubscriptionPolicy};
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
    sim_task: JoinHandle<()>,
    collector_task: JoinHandle<()>,
    sampler_task: JoinHandle<()>,
}

impl SimHandle {
    pub fn request_shutdown(&self) {
        let bus = self.bus.clone();
        tokio::spawn(async move {
            let _ = bus.shutdown().await;
        });
    }

    pub fn stop(&self) {
        self.request_shutdown();
        self.sim_task.abort();
        self.collector_task.abort();
        self.sampler_task.abort();
    }
}

/// Build the bus, register handlers, start publishing, and return handles.
pub fn launch_simulation(config: SimConfig, viz: Arc<Mutex<VisualizationState>>) -> SimHandle {
    let (tx, rx) = mpsc::unbounded_channel::<SimEvent>();

    // Build the event bus
    let mut builder = EventBus::builder();

    if config.bus.handler_timeout_ms > 0 {
        builder = builder.handler_timeout(Duration::from_millis(config.bus.handler_timeout_ms));
    }
    if config.bus.max_concurrent_async > 0 {
        builder = builder.max_concurrent_async(config.bus.max_concurrent_async);
    }
    builder = builder.default_subscription_policies(SubscriptionDefaults::default());

    let shutdown_timeout = Duration::from_millis(config.bus.shutdown_timeout_ms);
    builder = builder.shutdown_timeout(shutdown_timeout);

    let bus = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(builder.build())).expect("failed to build EventBus");

    let bus_clone = bus.clone();
    let tx_clone = tx.clone();
    let publish_config = config.publish.clone();
    let listeners = config.listeners.clone();
    let event_types = config.event_types.clone();
    let processed_counter = {
        let viz = viz.lock().expect("viz lock poisoned");
        viz.total_processed.clone()
    };

    // Spawn simulation task: registers handlers then runs publish loop
    let sim_task = tokio::spawn(async move {
        let listener_lookup: std::collections::HashMap<String, usize> =
            listeners.iter().enumerate().map(|(idx, cfg)| (cfg.name.clone(), idx)).collect();

        // Register handlers
        for (listener_idx, listener_cfg) in listeners.iter().cloned().enumerate() {
            let policy = build_subscription_policy(&listener_cfg);
            match listener_cfg.mode.clone() {
                HandlerMode::Async => {
                    let handler = MockAsyncHandler {
                        listener_idx,
                        cfg: listener_cfg,
                        tx: tx_clone.clone(),
                    };
                    let _ = bus_clone.subscribe_with_policy::<SimEnvelopeEvent, _, _>(handler, policy).await;
                }
                HandlerMode::Sync => {
                    let handler = MockSyncHandler {
                        listener_idx,
                        cfg: listener_cfg.clone(),
                        tx: tx_clone.clone(),
                    };
                    let no_retry = SyncSubscriptionPolicy {
                        priority: 0,
                        dead_letter: listener_cfg.dead_letter,
                    };
                    let _ = bus_clone.subscribe_with_policy::<SimEnvelopeEvent, _, _>(handler, no_retry).await;
                }
            }
        }

        // Register dead letter collector
        let dl_collector = DeadLetterCollector {
            tx: tx_clone.clone(),
            listener_lookup: Arc::new(listener_lookup),
        };
        let _ = bus_clone.subscribe_dead_letters(dl_collector).await;

        // Run publish loop
        let total_published = run_publish_loop(
            bus_clone.clone(),
            publish_config.clone(),
            event_types.clone(),
            processed_counter.clone(),
            tx_clone.clone(),
        )
        .await;

        match publish_config.stop_condition {
            StopCondition::TotalEvents(target_processed) => {
                while processed_counter.load(Ordering::Relaxed) < target_processed as u64 {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
            StopCondition::Duration(_) => loop {
                if let Ok(stats) = bus_clone.stats().await
                    && stats.in_flight_async == 0
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            },
        }

        let _ = tx_clone.send(SimEvent::PublishingStopped { total_published });
        let _ = tx_clone.send(SimEvent::SimulationDone);

        // Shutdown bus after publishing is done
        let _ = bus_clone.shutdown().await;
    });

    // Collector task
    let event_type_names = config.event_types.iter().map(|e| e.name.clone()).collect();
    let collector_task = tokio::spawn(run_collector(rx, Arc::clone(&viz), event_type_names, config.listeners.clone()));

    // Sampler task
    let sampler_task = tokio::spawn(run_sampler(Arc::clone(&viz), bus.clone()));

    SimHandle {
        bus,
        sim_task,
        collector_task,
        sampler_task,
    }
}

fn build_subscription_policy(cfg: &ListenerConfig) -> AsyncSubscriptionPolicy {
    let mut policy = AsyncSubscriptionPolicy::default()
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
