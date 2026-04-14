use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode};
use ratatui::DefaultTerminal;

use crate::config::types::SimConfig;
use crate::metrics::VisualizationState;
use crate::sim::runner::SimHandle;
use crate::ui::config_screen::ConfigState;
use crate::ui::dashboard::DashboardState;

pub enum AppPhase {
    Config(ConfigState),
    Running {
        viz: Arc<Mutex<VisualizationState>>,
        dashboard: DashboardState,
        sim_handle: SimHandle,
        config: SimConfig,
    },
    Quit,
}

pub struct App {
    pub phase: AppPhase,
}

impl App {
    pub fn new() -> Self {
        Self {
            phase: AppPhase::Config(ConfigState::new()),
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<(), Box<dyn std::error::Error>> {
        let tick_rate = Duration::from_millis(100);
        let mut last_tick = Instant::now();

        loop {
            // Draw
            terminal.draw(|frame| {
                let area = frame.area();
                let buf = frame.buffer_mut();
                match &self.phase {
                    AppPhase::Config(state) => {
                        crate::ui::config_screen::render_config_screen(area, buf, state);
                    }
                    AppPhase::Running { viz, dashboard, .. } => {
                        let viz = viz.lock().expect("viz lock poisoned");
                        crate::ui::dashboard::render_dashboard(area, buf, &viz, dashboard);
                    }
                    AppPhase::Quit => {}
                }
            })?;

            if matches!(self.phase, AppPhase::Quit) {
                break;
            }

            // Poll for input
            let timeout = tick_rate.saturating_sub(last_tick.elapsed());
            if event::poll(timeout)?
                && let Event::Key(key) = event::read()?
            {
                // Global quit
                if key.code == KeyCode::Char('q') || key.code == KeyCode::Char('Q') {
                    let mut request_quit = false;
                    let mut request_shutdown = false;
                    match &self.phase {
                        AppPhase::Config(state) => {
                            if !state.editing {
                                request_quit = true;
                            }
                        }
                        AppPhase::Running { .. } => {
                            request_shutdown = true;
                            request_quit = true;
                        }
                        AppPhase::Quit => break,
                    }
                    if request_shutdown {
                        self.shutdown_sim();
                    }
                    if request_quit {
                        self.phase = AppPhase::Quit;
                        continue;
                    }
                }

                enum PendingAction {
                    Start,
                    Rerun(SimConfig),
                    Reconfigure(SimConfig),
                }
                let mut pending_action: Option<PendingAction> = None;
                match &mut self.phase {
                    AppPhase::Config(state) => {
                        let should_start = state.handle_input(key);
                        if should_start {
                            pending_action = Some(PendingAction::Start);
                        }
                    }
                    AppPhase::Running { viz, dashboard, config, .. } => {
                        let viz_lock = viz.lock().expect("viz lock poisoned");
                        let is_done = viz_lock.sim_done;
                        dashboard.handle_input(key, &viz_lock);
                        drop(viz_lock);

                        if is_done && (key.code == KeyCode::Char('r') || key.code == KeyCode::Char('R')) {
                            pending_action = Some(PendingAction::Rerun(config.clone()));
                        }
                        if is_done && (key.code == KeyCode::Char('c') || key.code == KeyCode::Char('C')) {
                            pending_action = Some(PendingAction::Reconfigure(config.clone()));
                        }
                    }
                    AppPhase::Quit => {}
                }

                if let Some(action) = pending_action {
                    match action {
                        PendingAction::Start => self.start_simulation(),
                        PendingAction::Rerun(config) => {
                            self.shutdown_sim();
                            self.start_simulation_with_config(config);
                        }
                        PendingAction::Reconfigure(config) => {
                            self.shutdown_sim();
                            self.phase = AppPhase::Config(ConfigState::from_config(config));
                        }
                    }
                    continue;
                }
            }

            // Tick
            if last_tick.elapsed() >= tick_rate {
                last_tick = Instant::now();
            }
        }

        Ok(())
    }

    fn start_simulation(&mut self) {
        // Take config from the Config phase
        let config = match &self.phase {
            AppPhase::Config(state) => state.config.clone(),
            _ => return,
        };

        self.start_simulation_with_config(config);
    }

    fn start_simulation_with_config(&mut self, config: SimConfig) {
        let viz = Arc::new(Mutex::new(VisualizationState::new(&config.listeners)));

        {
            let mut v = viz.lock().expect("viz lock poisoned");
            v.sim_start = Some(Instant::now());
        }

        let sim_handle = crate::sim::runner::launch_simulation(config.clone(), Arc::clone(&viz));

        self.phase = AppPhase::Running {
            viz,
            dashboard: DashboardState::new(),
            sim_handle,
            config,
        };
    }

    fn shutdown_sim(&mut self) {
        if let AppPhase::Running { sim_handle, .. } = &mut self.phase {
            sim_handle.stop();
        }
    }
}
