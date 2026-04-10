use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode};
use ratatui::DefaultTerminal;

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
                    match &mut self.phase {
                        AppPhase::Config(state) => {
                            if !state.editing {
                                self.phase = AppPhase::Quit;
                                continue;
                            }
                        }
                        AppPhase::Running { .. } => {
                            self.shutdown_sim();
                            self.phase = AppPhase::Quit;
                            continue;
                        }
                        AppPhase::Quit => break,
                    }
                }

                match &mut self.phase {
                    AppPhase::Config(state) => {
                        let should_start = state.handle_input(key);
                        if should_start {
                            self.start_simulation();
                        }
                    }
                    AppPhase::Running { viz, dashboard, .. } => {
                        let viz = viz.lock().expect("viz lock poisoned");
                        dashboard.handle_input(key, &viz);
                    }
                    AppPhase::Quit => {}
                }
            }

            // Tick: update flow blips
            if last_tick.elapsed() >= tick_rate {
                if let AppPhase::Running { viz, .. } = &self.phase {
                    let mut viz = viz.lock().expect("viz lock poisoned");
                    viz.tick_flow_blips(tick_rate.as_secs_f64());
                }
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

        let viz = Arc::new(Mutex::new(VisualizationState::new(config.bus.buffer_size)));
        {
            let mut v = viz.lock().expect("viz lock poisoned");
            v.sim_start = Some(Instant::now());
        }

        let sim_handle = crate::sim::runner::launch_simulation(config.clone(), Arc::clone(&viz));

        self.phase = AppPhase::Running {
            viz,
            dashboard: DashboardState::new(),
            sim_handle,
        };
    }

    fn shutdown_sim(&mut self) {
        if let AppPhase::Running { sim_handle, .. } = &mut self.phase {
            sim_handle.request_shutdown();
        }
    }
}
