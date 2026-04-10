use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Sparkline, Table, Widget};

use crate::metrics::VisualizationState;
use crate::ui::widgets::{flow, pressure, throughput_chart};

pub struct DashboardState {
    pub dead_letter_scroll: usize,
}

impl DashboardState {
    pub fn new() -> Self {
        Self { dead_letter_scroll: 0 }
    }

    pub fn handle_input(&mut self, key: KeyEvent, viz: &VisualizationState) {
        match key.code {
            KeyCode::Up => {
                if self.dead_letter_scroll > 0 {
                    self.dead_letter_scroll -= 1;
                }
            }
            KeyCode::Down => {
                let max = viz.dead_letters.len().saturating_sub(1);
                if self.dead_letter_scroll < max {
                    self.dead_letter_scroll += 1;
                }
            }
            _ => {}
        }
    }
}

pub fn render_dashboard(area: Rect, buf: &mut Buffer, viz: &VisualizationState, dash: &DashboardState) {
    let [header_area, main_area, throughput_chart_area, dead_letter_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(12), Constraint::Min(4), Constraint::Length(8)]).areas(area);

    // Header
    render_header(header_area, buf, viz);

    // Main area: counters (left) + pressure/sparklines (right)
    let [left_col, right_col] = Layout::horizontal([Constraint::Length(28), Constraint::Min(0)]).areas(main_area);

    render_counters(left_col, buf, viz);
    render_right_panel(right_col, buf, viz);

    // Throughput chart
    throughput_chart::render_throughput_chart(throughput_chart_area, buf, viz);

    // Dead letters
    render_dead_letters(dead_letter_area, buf, viz, dash);
}

fn render_header(area: Rect, buf: &mut Buffer, viz: &VisualizationState) {
    let status = if viz.sim_done { "DONE" } else { "RUNNING" };
    let status_color = if viz.sim_done { Color::Yellow } else { Color::Green };

    let line = Line::from(vec![
        Span::styled("  jaeb-visualizer  ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled(status, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        Span::raw(format!("  elapsed: {:.1}s  rate: {:.1}/s", viz.elapsed_secs(), viz.current_rate())),
        Span::styled("   Q=quit  ↑↓=scroll dead letters", Style::default().fg(Color::DarkGray)),
    ]);
    Paragraph::new(line).render(area, buf);
}

fn render_counters(area: Rect, buf: &mut Buffer, viz: &VisualizationState) {
    let block = Block::default().title(" Counters ").borders(Borders::ALL);
    let inner = block.inner(area);
    block.render(area, buf);

    let lines = vec![
        counter_line("Published:", viz.total_published, Color::White),
        counter_line("Handled:", viz.total_handled, Color::Green),
        counter_line("Failed:", viz.total_failed, Color::Red),
        counter_line("Dead Letters:", viz.total_dead_letters, Color::Magenta),
        counter_line("In-flight:", viz.in_flight.max(0) as u64, Color::Yellow),
        counter_line("Async tasks:", viz.bus_in_flight_async as u64, Color::Cyan),
        counter_line("Subscriptions:", viz.bus_total_subscriptions as u64, Color::DarkGray),
        counter_line("BP hits:", viz.backpressure_hits, Color::Red),
    ];
    Paragraph::new(lines).render(inner, buf);
}

fn counter_line(label: &str, value: u64, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {:<16}", label), Style::default().fg(Color::White)),
        Span::styled(format!("{:>8}", value), Style::default().fg(color).add_modifier(Modifier::BOLD)),
    ])
}

fn render_right_panel(area: Rect, buf: &mut Buffer, viz: &VisualizationState) {
    let [pressure_area, flow_area, throughput_area, bp_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Length(4), Constraint::Min(3), Constraint::Length(3)]).areas(area);

    // Pressure gauge
    pressure::render_pressure(pressure_area, buf, viz);

    // Event flow animation
    flow::render_flow(flow_area, buf, viz);

    // Throughput sparkline
    let tp_data: Vec<u64> = viz.throughput_history.iter().copied().collect();
    let tp_block = Block::default().title(" Throughput ").borders(Borders::ALL);
    let tp_inner = tp_block.inner(throughput_area);
    tp_block.render(throughput_area, buf);
    Sparkline::default()
        .data(&tp_data)
        .style(Style::default().fg(Color::Cyan))
        .render(tp_inner, buf);

    // Backpressure sparkline
    let bp_data: Vec<u64> = viz.backpressure_history.iter().copied().collect();
    let bp_block = Block::default().title(" Backpressure ").borders(Borders::ALL);
    let bp_inner = bp_block.inner(bp_area);
    bp_block.render(bp_area, buf);
    Sparkline::default()
        .data(&bp_data)
        .style(Style::default().fg(Color::Red))
        .render(bp_inner, buf);
}

fn render_dead_letters(area: Rect, buf: &mut Buffer, viz: &VisualizationState, dash: &DashboardState) {
    let block = Block::default().title(" Dead Letters ").borders(Borders::ALL);
    let inner = block.inner(area);
    block.render(area, buf);

    if viz.dead_letters.is_empty() {
        Paragraph::new(Line::from(Span::styled("  No dead letters yet", Style::default().fg(Color::DarkGray)))).render(inner, buf);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Time"),
        Cell::from("Listener"),
        Cell::from("Event"),
        Cell::from("Seq"),
        Cell::from("Reason"),
    ])
    .style(Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = viz
        .dead_letters
        .iter()
        .skip(dash.dead_letter_scroll)
        .map(|dl| {
            Row::new(vec![
                Cell::from(format!("{:.2}s", dl.elapsed_secs)),
                Cell::from(dl.listener.clone()),
                Cell::from(dl.event_type.clone()),
                Cell::from(format!("#{}", dl.seq)),
                Cell::from(dl.reason.clone()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(8),
        Constraint::Length(16),
        Constraint::Length(14),
        Constraint::Length(8),
        Constraint::Min(20),
    ];

    Table::new(rows, widths)
        .header(header)
        .style(Style::default().fg(Color::White))
        .render(inner, buf);
}
