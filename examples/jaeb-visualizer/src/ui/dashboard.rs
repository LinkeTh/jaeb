use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Widget};

use crate::metrics::VisualizationState;
use crate::ui::palette;
use crate::ui::widgets::{inflight_chart, latency_chart, throughput_chart};

pub struct DashboardState {
    pub dead_letter_scroll: usize,
    pub handler_scroll: usize,
}

impl DashboardState {
    pub fn new() -> Self {
        Self {
            dead_letter_scroll: 0,
            handler_scroll: 0,
        }
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
            KeyCode::PageUp => {
                self.handler_scroll = self.handler_scroll.saturating_sub(1);
            }
            KeyCode::PageDown => {
                if self.handler_scroll + 1 < viz.handler_metrics.len() {
                    self.handler_scroll += 1;
                }
            }
            _ => {}
        }
    }
}

pub fn render_dashboard(area: Rect, buf: &mut Buffer, viz: &VisualizationState, dash: &DashboardState) {
    buf.set_style(area, Style::default().bg(palette::BG));

    let [header_area, summary_area, charts_area, dead_letter_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(10), Constraint::Min(8), Constraint::Length(8)]).areas(area);

    render_header(header_area, buf, viz);

    let [left_col, right_col] = Layout::horizontal([Constraint::Length(34), Constraint::Min(0)]).areas(summary_area);
    render_counters(left_col, buf, viz);
    render_right_panel(right_col, buf, viz);

    render_handler_panels(charts_area, buf, viz, dash);
    render_dead_letters(dead_letter_area, buf, viz, dash);
}

fn render_header(area: Rect, buf: &mut Buffer, viz: &VisualizationState) {
    let status = if viz.sim_done {
        "DONE"
    } else if viz.publishing_stopped {
        "DRAINING"
    } else {
        "RUNNING"
    };
    let status_color = if viz.sim_done {
        palette::DONE
    } else if viz.publishing_stopped {
        palette::WARN
    } else {
        palette::RUNNING
    };
    let suffix = if viz.sim_done { "*" } else { "" };
    let hint = if viz.sim_done {
        "   Q=quit  R=rerun  C=config  ↑↓ dead letters  PgUp/PgDn handlers"
    } else {
        "   Q=quit  ↑↓ dead letters  PgUp/PgDn handlers"
    };

    let line = Line::from(vec![
        Span::styled("  jaeb-visualizer  ", Style::default().fg(palette::BRAND).add_modifier(Modifier::BOLD)),
        Span::styled(status, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        Span::raw(format!(
            "  elapsed: {:.1}s{}  avg: {:.1} ev/s{}",
            viz.elapsed_secs(),
            suffix,
            viz.current_rate(),
            suffix
        )),
        Span::styled(hint, Style::default().fg(palette::RUNNING).add_modifier(Modifier::BOLD)),
    ]);
    Paragraph::new(line).render(area, buf);
}

fn render_counters(area: Rect, buf: &mut Buffer, viz: &VisualizationState) {
    let block = Block::default()
        .title(" Totals ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER))
        .style(Style::default().bg(palette::PANEL_BG));
    let inner = block.inner(area);
    block.render(area, buf);

    let lines = vec![
        counter_line("Published:", viz.total_published, palette::VALUE),
        counter_line("Handled:", viz.total_handled, palette::RUNNING),
        counter_line("Failed:", viz.total_failed, palette::ERROR),
        counter_line("Dead Letters:", viz.total_dead_letters, palette::WARN),
        counter_line("In-flight:", viz.in_flight.max(0) as u64, palette::DONE),
        counter_line("Async tasks:", viz.bus_in_flight_async as u64, palette::BRAND),
        counter_line("Subscriptions:", viz.bus_total_subscriptions as u64, palette::MUTED),
        counter_line("BP hits:", viz.backpressure_hits, palette::ERROR),
    ];
    Paragraph::new(lines).render(inner, buf);
}

fn counter_line(label: &str, value: u64, color: ratatui::style::Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {:<16}", label), Style::default().fg(palette::MUTED)),
        Span::styled(format!("{:>8}", value), Style::default().fg(color).add_modifier(Modifier::BOLD)),
    ])
}

fn render_right_panel(area: Rect, buf: &mut Buffer, viz: &VisualizationState) {
    let [latency_area, inflight_area] = Layout::horizontal([Constraint::Length(24), Constraint::Min(0)]).areas(area);
    render_latency_summary(latency_area, buf, viz);
    inflight_chart::render_inflight(inflight_area, buf, viz);
}

fn render_latency_summary(area: Rect, buf: &mut Buffer, viz: &VisualizationState) {
    let block = Block::default()
        .title(" Latency (handler avg) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER_ALT))
        .style(Style::default().bg(palette::PANEL_BG));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height < 1 {
        return;
    }

    let avg_str = match viz.avg_latency_ms() {
        Some(v) => format!("{v:.1} ms"),
        None => "-".into(),
    };
    let min_str = match viz.global_min_latency_ms() {
        Some(v) => format!("{v:.0} ms"),
        None => "-".into(),
    };
    let max_ms = viz.global_max_latency_ms();
    let max_str = if max_ms > 0.0 { format!("{max_ms:.0} ms") } else { "-".into() };

    let lines = vec![
        Line::from(vec![
            Span::styled(format!("  {:<8}", "avg:"), Style::default().fg(palette::MUTED)),
            Span::styled(
                format!("{:>10}", avg_str),
                Style::default().fg(palette::VALUE).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("  {:<8}", "min:"), Style::default().fg(palette::MUTED)),
            Span::styled(
                format!("{:>10}", min_str),
                Style::default().fg(palette::RUNNING).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("  {:<8}", "max:"), Style::default().fg(palette::MUTED)),
            Span::styled(
                format!("{:>10}", max_str),
                Style::default().fg(palette::WARN).add_modifier(Modifier::BOLD),
            ),
        ]),
    ];
    Paragraph::new(lines).render(inner, buf);
}

fn render_handler_panels(area: Rect, buf: &mut Buffer, viz: &VisualizationState, dash: &DashboardState) {
    let rows = viz.handler_metrics.len();
    if rows == 0 {
        Block::default()
            .title(" Per-handler Throughput & Latency ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_ALT))
            .render(area, buf);
        return;
    }

    let legend_height: u16 = 1;
    let available_rows_height = area.height.saturating_sub(legend_height).max(1);
    let remaining_rows = rows.saturating_sub(dash.handler_scroll);

    let min_row_height: u16 = 5;
    let target_visible_rows: usize = remaining_rows.clamp(1, 3);
    let mut visible_rows = target_visible_rows;
    while visible_rows > 1 && available_rows_height < min_row_height * visible_rows as u16 {
        visible_rows -= 1;
    }

    let base_height = available_rows_height / visible_rows as u16;
    let remainder = available_rows_height % visible_rows as u16;

    let mut constraints = Vec::with_capacity(visible_rows + 1);
    for idx in 0..visible_rows {
        let extra = if idx < remainder as usize { 1 } else { 0 };
        constraints.push(Constraint::Length(base_height + extra));
    }
    constraints.push(Constraint::Length(legend_height));

    let chunks = Layout::vertical(constraints).split(area);
    let visible_rows = chunks.len().saturating_sub(1).min(remaining_rows);
    for (idx, handler) in viz.handler_metrics.iter().skip(dash.handler_scroll).take(visible_rows).enumerate() {
        let [throughput_area, lat_area] = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(chunks[idx]);
        let color_idx = dash.handler_scroll + idx;
        throughput_chart::render_handler_throughput(throughput_area, buf, handler, color_idx, viz.sim_done);
        latency_chart::render_handler_latency(lat_area, buf, handler, color_idx, viz.sim_done);
    }

    draw_handler_scrollbar(area, buf, rows, visible_rows, dash.handler_scroll);

    let hidden = rows.saturating_sub(dash.handler_scroll + visible_rows);
    let legend = Line::from(vec![
        Span::styled("  Legend: ", Style::default().fg(palette::WARN)),
        Span::styled("ev/s", Style::default().fg(palette::BRAND).add_modifier(Modifier::BOLD)),
        Span::styled(" throughput (left)  ", Style::default().fg(palette::MUTED)),
        Span::styled("ms", Style::default().fg(palette::VALUE).add_modifier(Modifier::BOLD)),
        Span::styled(" avg latency (right)  ", Style::default().fg(palette::MUTED)),
        Span::styled("0.5s samples", Style::default().fg(palette::MUTED)),
        Span::styled(
            if hidden > 0 {
                format!("  scroll PgDn ({hidden} more)")
            } else if dash.handler_scroll > 0 {
                format!("  scroll PgUp ({})", dash.handler_scroll)
            } else {
                String::new()
            },
            Style::default().fg(palette::WARN),
        ),
    ]);
    Paragraph::new(legend).render(chunks[chunks.len().saturating_sub(1)], buf);
}

fn draw_handler_scrollbar(area: Rect, buf: &mut Buffer, total: usize, visible: usize, scroll: usize) {
    if total <= visible || area.width == 0 || area.height < 3 {
        return;
    }

    let x = area.x + area.width.saturating_sub(1);
    let top = area.y;
    let bottom = area.y + area.height.saturating_sub(2);
    let track_h = bottom.saturating_sub(top).saturating_add(1);
    if track_h == 0 {
        return;
    }

    for y in top..=bottom {
        buf.set_string(x, y, "|", Style::default().fg(palette::MUTED));
    }

    let thumb_h = ((visible as f64 / total as f64) * track_h as f64).ceil().max(1.0) as u16;
    let max_scroll = total.saturating_sub(visible).max(1);
    let max_y_offset = track_h.saturating_sub(thumb_h);
    let thumb_offset = ((scroll as f64 / max_scroll as f64) * max_y_offset as f64).round() as u16;
    let thumb_top = top + thumb_offset;
    let thumb_bottom = (thumb_top + thumb_h.saturating_sub(1)).min(bottom);
    for y in thumb_top..=thumb_bottom {
        buf.set_string(x, y, "#", Style::default().fg(palette::BRAND).add_modifier(Modifier::BOLD));
    }
}

fn render_dead_letters(area: Rect, buf: &mut Buffer, viz: &VisualizationState, dash: &DashboardState) {
    let block = Block::default()
        .title(" Dead Letters ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER))
        .style(Style::default().bg(palette::PANEL_BG));
    let inner = block.inner(area);
    block.render(area, buf);

    if viz.dead_letters.is_empty() {
        Paragraph::new(Line::from(Span::styled("  No dead letters yet", Style::default().fg(palette::MUTED)))).render(inner, buf);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Time"),
        Cell::from("Listener"),
        Cell::from("Event"),
        Cell::from("Seq"),
        Cell::from("Reason"),
    ])
    .style(Style::default().fg(palette::WARN).add_modifier(Modifier::BOLD));

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
        .style(Style::default().fg(palette::VALUE))
        .render(inner, buf);
}
