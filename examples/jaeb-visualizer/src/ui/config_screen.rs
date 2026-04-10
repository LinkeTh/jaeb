use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::config::types::*;

// ── Config tabs ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConfigTab {
    Bus,
    EventTypes,
    Listeners,
    Publish,
}

impl ConfigTab {
    const ALL: [ConfigTab; 4] = [ConfigTab::Bus, ConfigTab::EventTypes, ConfigTab::Listeners, ConfigTab::Publish];

    fn label(&self) -> &'static str {
        match self {
            Self::Bus => "Bus",
            Self::EventTypes => "Event Types",
            Self::Listeners => "Listeners",
            Self::Publish => "Publish",
        }
    }

    fn next(&self) -> Self {
        let idx = Self::ALL.iter().position(|t| t == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    fn prev(&self) -> Self {
        let idx = Self::ALL.iter().position(|t| t == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

// ── Config state ─────────────────────────────────────────────────────────

pub struct ConfigState {
    pub config: SimConfig,
    pub active_tab: ConfigTab,
    pub field_idx: usize,
    pub selected_listener: usize,
    pub editing: bool,
    pub edit_buffer: String,
    pub error_msg: Option<String>,
}

impl ConfigState {
    pub fn new() -> Self {
        Self {
            config: SimConfig::default(),
            active_tab: ConfigTab::Bus,
            field_idx: 0,
            selected_listener: 0,
            editing: false,
            edit_buffer: String::new(),
            error_msg: None,
        }
    }

    fn field_count(&self) -> usize {
        match self.active_tab {
            ConfigTab::Bus => 4,
            ConfigTab::EventTypes => 2,
            ConfigTab::Listeners => {
                if self.config.listeners.is_empty() {
                    0
                } else {
                    8 // fields per listener
                }
            }
            ConfigTab::Publish => 4,
        }
    }

    /// Returns true if the user pressed S and config is valid (should start sim).
    pub fn handle_input(&mut self, key: KeyEvent) -> bool {
        self.error_msg = None;

        match key.code {
            // Tab navigation
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.active_tab = self.active_tab.prev();
                } else {
                    self.active_tab = self.active_tab.next();
                }
                self.field_idx = 0;
                self.editing = false;
            }
            // Start simulation
            KeyCode::Char('s') | KeyCode::Char('S') if !self.editing => match self.config.validate() {
                Ok(()) => return true,
                Err(msg) => self.error_msg = Some(msg),
            },
            // Add listener
            KeyCode::Char('a') | KeyCode::Char('A') if !self.editing && self.active_tab == ConfigTab::Listeners => {
                if self.config.listeners.len() < 6 {
                    let idx = self.config.listeners.len() + 1;
                    let l = ListenerConfig {
                        name: format!("listener_{}", idx),
                        ..Default::default()
                    };
                    self.config.listeners.push(l);
                    self.selected_listener = self.config.listeners.len() - 1;
                    self.field_idx = 0;
                } else {
                    self.error_msg = Some("Maximum 6 listeners".into());
                }
            }
            // Delete listener
            KeyCode::Char('d') | KeyCode::Char('D') if !self.editing && self.active_tab == ConfigTab::Listeners => {
                if !self.config.listeners.is_empty() {
                    self.config.listeners.remove(self.selected_listener);
                    if self.selected_listener > 0 && self.selected_listener >= self.config.listeners.len() {
                        self.selected_listener = self.config.listeners.len().saturating_sub(1);
                    }
                }
            }
            // Select listener (left/right in Listeners tab when not editing)
            KeyCode::Left if !self.editing && self.active_tab == ConfigTab::Listeners && self.field_idx == 0 => {
                if self.selected_listener > 0 {
                    self.selected_listener -= 1;
                }
            }
            KeyCode::Right if !self.editing && self.active_tab == ConfigTab::Listeners && self.field_idx == 0 => {
                if self.selected_listener + 1 < self.config.listeners.len() {
                    self.selected_listener += 1;
                }
            }
            // Field navigation
            KeyCode::Up => {
                if self.field_idx > 0 {
                    self.field_idx -= 1;
                }
                self.editing = false;
            }
            KeyCode::Down => {
                let count = self.field_count();
                if count > 0 && self.field_idx < count - 1 {
                    self.field_idx += 1;
                }
                self.editing = false;
            }
            // Enter: start editing the current field or toggle enum
            KeyCode::Enter => {
                if self.editing {
                    self.commit_edit();
                    self.editing = false;
                } else {
                    self.start_edit_or_toggle();
                }
            }
            // Space: cycle enum fields
            KeyCode::Char(' ') if !self.editing => {
                self.cycle_enum_field();
            }
            // Editing: character input
            KeyCode::Char(c) if self.editing => {
                self.edit_buffer.push(c);
            }
            KeyCode::Backspace if self.editing => {
                self.edit_buffer.pop();
            }
            // Escape: cancel edit
            KeyCode::Esc => {
                self.editing = false;
            }
            _ => {}
        }
        false
    }

    fn start_edit_or_toggle(&mut self) {
        match self.active_tab {
            ConfigTab::Bus => {
                let val = match self.field_idx {
                    0 => self.config.bus.buffer_size.to_string(),
                    1 => self.config.bus.handler_timeout_ms.to_string(),
                    2 => self.config.bus.max_concurrent_async.to_string(),
                    3 => self.config.bus.shutdown_timeout_ms.to_string(),
                    _ => return,
                };
                self.edit_buffer = val;
                self.editing = true;
            }
            ConfigTab::EventTypes => {
                let val = match self.field_idx {
                    0 => self.config.event_types[0].clone(),
                    1 => self.config.event_types[1].clone(),
                    _ => return,
                };
                self.edit_buffer = val;
                self.editing = true;
            }
            ConfigTab::Listeners => {
                if self.config.listeners.is_empty() {
                    return;
                }
                let l = &self.config.listeners[self.selected_listener];
                match self.field_idx {
                    0 => {
                        self.edit_buffer = l.name.clone();
                        self.editing = true;
                    }
                    1 => self.cycle_enum_field(), // event_type_idx
                    2 => self.cycle_enum_field(), // mode
                    3 => {
                        self.edit_buffer = l.processing_ms.to_string();
                        self.editing = true;
                    }
                    4 => {
                        self.edit_buffer = format!("{:.2}", l.failure_rate);
                        self.editing = true;
                    }
                    5 => {
                        self.edit_buffer = l.max_retries.to_string();
                        self.editing = true;
                    }
                    6 => self.cycle_enum_field(), // retry strategy
                    7 => self.cycle_enum_field(), // dead_letter
                    _ => {}
                }
            }
            ConfigTab::Publish => {
                match self.field_idx {
                    0 => {
                        self.edit_buffer = format!("{:.1}", self.config.publish.events_per_sec);
                        self.editing = true;
                    }
                    1 => self.cycle_enum_field(), // stop condition type
                    2 => {
                        let val = match &self.config.publish.stop_condition {
                            StopCondition::Duration(d) => d.as_secs().to_string(),
                            StopCondition::TotalEvents(n) => n.to_string(),
                        };
                        self.edit_buffer = val;
                        self.editing = true;
                    }
                    3 => {
                        self.edit_buffer = self.config.publish.burst_every_n.to_string();
                        self.editing = true;
                    }
                    _ => {}
                }
            }
        }
    }

    fn cycle_enum_field(&mut self) {
        match self.active_tab {
            ConfigTab::Listeners if !self.config.listeners.is_empty() => {
                let l = &mut self.config.listeners[self.selected_listener];
                match self.field_idx {
                    1 => l.event_type_idx = 1 - l.event_type_idx,
                    2 => l.mode = l.mode.cycle_next(),
                    6 => l.retry_strategy = l.retry_strategy.cycle_next(),
                    7 => l.dead_letter = !l.dead_letter,
                    _ => {}
                }
            }
            ConfigTab::Publish if self.field_idx == 1 => {
                self.config.publish.stop_condition = self.config.publish.stop_condition.cycle_next();
            }
            _ => {}
        }
    }

    fn commit_edit(&mut self) {
        match self.active_tab {
            ConfigTab::Bus => {
                if let Ok(val) = self.edit_buffer.parse::<usize>() {
                    match self.field_idx {
                        0 => self.config.bus.buffer_size = val,
                        2 => self.config.bus.max_concurrent_async = val,
                        _ => {}
                    }
                }
                if let Ok(val) = self.edit_buffer.parse::<u64>() {
                    match self.field_idx {
                        1 => self.config.bus.handler_timeout_ms = val,
                        3 => self.config.bus.shutdown_timeout_ms = val,
                        _ => {}
                    }
                }
            }
            ConfigTab::EventTypes => match self.field_idx {
                0 => self.config.event_types[0] = self.edit_buffer.clone(),
                1 => self.config.event_types[1] = self.edit_buffer.clone(),
                _ => {}
            },
            ConfigTab::Listeners if !self.config.listeners.is_empty() => {
                let l = &mut self.config.listeners[self.selected_listener];
                match self.field_idx {
                    0 => l.name = self.edit_buffer.clone(),
                    3 => {
                        if let Ok(v) = self.edit_buffer.parse() {
                            l.processing_ms = v;
                        }
                    }
                    4 => {
                        if let Ok(v) = self.edit_buffer.parse::<f64>() {
                            l.failure_rate = v.clamp(0.0, 1.0);
                        }
                    }
                    5 => {
                        if let Ok(v) = self.edit_buffer.parse() {
                            l.max_retries = v;
                        }
                    }
                    _ => {}
                }
            }
            ConfigTab::Publish => match self.field_idx {
                0 => {
                    if let Ok(v) = self.edit_buffer.parse::<f64>() {
                        self.config.publish.events_per_sec = v.max(0.1);
                    }
                }
                2 => {
                    if let Ok(v) = self.edit_buffer.parse::<u64>() {
                        match &self.config.publish.stop_condition {
                            StopCondition::Duration(_) => {
                                self.config.publish.stop_condition = StopCondition::Duration(std::time::Duration::from_secs(v));
                            }
                            StopCondition::TotalEvents(_) => {
                                self.config.publish.stop_condition = StopCondition::TotalEvents(v as usize);
                            }
                        }
                    }
                }
                3 => {
                    if let Ok(v) = self.edit_buffer.parse() {
                        self.config.publish.burst_every_n = v;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

// ── Rendering ────────────────────────────────────────────────────────────

pub fn render_config_screen(area: Rect, buf: &mut Buffer, state: &ConfigState) {
    let [header_area, tabs_area, content_area, help_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0), Constraint::Length(2)]).areas(area);

    // Header
    Paragraph::new(Line::from(vec![
        Span::styled("  jaeb-visualizer", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("  —  Configure Simulation"),
    ]))
    .render(header_area, buf);

    // Tabs
    render_tabs(tabs_area, buf, state.active_tab);

    // Content
    let content = Block::default().borders(Borders::ALL).title(format!(" {} ", state.active_tab.label()));
    let inner = content.inner(content_area);
    content.render(content_area, buf);

    match state.active_tab {
        ConfigTab::Bus => render_bus_tab(inner, buf, state),
        ConfigTab::EventTypes => render_event_types_tab(inner, buf, state),
        ConfigTab::Listeners => render_listeners_tab(inner, buf, state),
        ConfigTab::Publish => render_publish_tab(inner, buf, state),
    }

    // Help line
    let help_text = if let Some(ref err) = state.error_msg {
        Line::from(vec![Span::styled(format!("  Error: {}", err), Style::default().fg(Color::Red))])
    } else {
        Line::from(vec![Span::styled(
            "  ↑↓ fields  Tab/Shift+Tab sections  Enter edit  Space toggle  S start  Q quit",
            Style::default().fg(Color::DarkGray),
        )])
    };
    Paragraph::new(help_text).render(help_area, buf);
}

fn render_tabs(area: Rect, buf: &mut Buffer, active: ConfigTab) {
    let mut spans = vec![Span::raw("  ")];
    for tab in ConfigTab::ALL {
        let style = if tab == active {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(format!(" {} ", tab.label()), style));
        spans.push(Span::raw(" "));
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

fn field_line(label: &str, value: &str, selected: bool, editing: bool, edit_buf: &str) -> Line<'static> {
    let label_style = Style::default().fg(Color::White);
    let value_str = if editing && selected {
        format!("{}_", edit_buf)
    } else {
        value.to_string()
    };
    let val_style = if selected {
        if editing {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::REVERSED)
        }
    } else {
        Style::default().fg(Color::White)
    };
    let marker = if selected { "► " } else { "  " };
    Line::from(vec![
        Span::styled(marker.to_string(), Style::default().fg(Color::Cyan)),
        Span::styled(format!("{:<24}", label), label_style),
        Span::styled(value_str, val_style),
    ])
}

fn render_bus_tab(area: Rect, buf: &mut Buffer, state: &ConfigState) {
    let c = &state.config.bus;
    let lines = vec![
        field_line(
            "Buffer Size:",
            &c.buffer_size.to_string(),
            state.field_idx == 0,
            state.editing,
            &state.edit_buffer,
        ),
        field_line(
            "Handler Timeout (ms):",
            &if c.handler_timeout_ms == 0 {
                "none".into()
            } else {
                c.handler_timeout_ms.to_string()
            },
            state.field_idx == 1,
            state.editing,
            &state.edit_buffer,
        ),
        field_line(
            "Max Concurrent Async:",
            &if c.max_concurrent_async == 0 {
                "unlimited".into()
            } else {
                c.max_concurrent_async.to_string()
            },
            state.field_idx == 2,
            state.editing,
            &state.edit_buffer,
        ),
        field_line(
            "Shutdown Timeout (ms):",
            &c.shutdown_timeout_ms.to_string(),
            state.field_idx == 3,
            state.editing,
            &state.edit_buffer,
        ),
    ];
    Paragraph::new(lines).render(area, buf);
}

fn render_event_types_tab(area: Rect, buf: &mut Buffer, state: &ConfigState) {
    let lines = vec![
        field_line(
            "Event Type A:",
            &state.config.event_types[0],
            state.field_idx == 0,
            state.editing,
            &state.edit_buffer,
        ),
        field_line(
            "Event Type B:",
            &state.config.event_types[1],
            state.field_idx == 1,
            state.editing,
            &state.edit_buffer,
        ),
    ];
    Paragraph::new(lines).render(area, buf);
}

fn render_listeners_tab(area: Rect, buf: &mut Buffer, state: &ConfigState) {
    if state.config.listeners.is_empty() {
        let lines = vec![Line::from(Span::styled(
            "  No listeners. Press A to add one.",
            Style::default().fg(Color::DarkGray),
        ))];
        Paragraph::new(lines).render(area, buf);
        return;
    }

    let [list_area, edit_area] = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);

    // Listener list (horizontal)
    let mut spans = vec![Span::raw("  ")];
    for (i, l) in state.config.listeners.iter().enumerate() {
        let style = if i == state.selected_listener {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(Color::White)
        };
        spans.push(Span::styled(format!(" {} ", l.name), style));
        spans.push(Span::raw(" "));
    }
    let list_lines = vec![
        Line::from(spans),
        Line::from(Span::styled("  ←→ select  A add  D delete", Style::default().fg(Color::DarkGray))),
    ];
    Paragraph::new(list_lines).render(list_area, buf);

    // Edit panel for selected listener
    let l = &state.config.listeners[state.selected_listener];
    let et_name = &state.config.event_types[l.event_type_idx as usize];
    let lines = vec![
        field_line("Name:", &l.name, state.field_idx == 0, state.editing, &state.edit_buffer),
        field_line(
            "Event Type:",
            &format!("{} ({})", et_name, l.event_type_idx),
            state.field_idx == 1,
            false,
            "",
        ),
        field_line("Mode:", &l.mode.to_string(), state.field_idx == 2, false, ""),
        field_line(
            "Processing (ms):",
            &l.processing_ms.to_string(),
            state.field_idx == 3,
            state.editing,
            &state.edit_buffer,
        ),
        field_line(
            "Failure Rate:",
            &format!("{:.2}", l.failure_rate),
            state.field_idx == 4,
            state.editing,
            &state.edit_buffer,
        ),
        field_line(
            "Max Retries:",
            &l.max_retries.to_string(),
            state.field_idx == 5,
            state.editing,
            &state.edit_buffer,
        ),
        field_line("Retry Strategy:", &l.retry_strategy.to_string(), state.field_idx == 6, false, ""),
        field_line("Dead Letter:", if l.dead_letter { "Yes" } else { "No" }, state.field_idx == 7, false, ""),
    ];
    Paragraph::new(lines).render(edit_area, buf);
}

fn render_publish_tab(area: Rect, buf: &mut Buffer, state: &ConfigState) {
    let c = &state.config.publish;
    let stop_type = if c.stop_condition.is_duration() { "Duration" } else { "Total Events" };
    let stop_val = match &c.stop_condition {
        StopCondition::Duration(d) => format!("{} seconds", d.as_secs()),
        StopCondition::TotalEvents(n) => format!("{} events", n),
    };
    let lines = vec![
        field_line(
            "Events/sec:",
            &format!("{:.1}", c.events_per_sec),
            state.field_idx == 0,
            state.editing,
            &state.edit_buffer,
        ),
        field_line("Stop Mode:", stop_type, state.field_idx == 1, false, ""),
        field_line("Stop Value:", &stop_val, state.field_idx == 2, state.editing, &state.edit_buffer),
        field_line(
            "Burst Every N:",
            &if c.burst_every_n == 0 {
                "disabled".into()
            } else {
                c.burst_every_n.to_string()
            },
            state.field_idx == 3,
            state.editing,
            &state.edit_buffer,
        ),
    ];
    Paragraph::new(lines).render(area, buf);
}
