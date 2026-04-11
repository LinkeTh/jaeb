use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Widget};

use crate::metrics::state::HandlerViz;
use crate::ui::palette;

const SAMPLE_SECS: f64 = 0.5;
const WINDOW_SAMPLES: usize = 60;
const WINDOW_SECS: f64 = SAMPLE_SECS * WINDOW_SAMPLES as f64;

pub fn render_handler_throughput(area: Rect, buf: &mut Buffer, handler: &HandlerViz, color_idx: usize, done: bool) {
    if area.height < 4 {
        Block::default()
            .title(format!(" {} | {:.1} ev/s ", handler.name, handler.current_rate()))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_ALT))
            .style(Style::default().bg(palette::PANEL_BG))
            .render(area, buf);
        return;
    }

    let history_len = handler.throughput_history.len();

    let points: Vec<(f64, f64)> = handler
        .throughput_history
        .iter()
        .enumerate()
        .map(|(idx, v)| {
            let age_samples = history_len.saturating_sub(1).saturating_sub(idx) as f64;
            let x = -(age_samples * SAMPLE_SECS);
            let y = *v as f64 / SAMPLE_SECS;
            (x, y)
        })
        .collect();

    let fallback = [(-WINDOW_SECS, 0.0), (0.0, 0.0)];
    let data = if points.is_empty() { &fallback[..] } else { points.as_slice() };

    let peak_per_sec = handler
        .throughput_history
        .iter()
        .copied()
        .max()
        .map(|v| v as f64 / SAMPLE_SECS)
        .unwrap_or(1.0)
        .max(1.0);
    let y_max = (peak_per_sec * 1.2).ceil().max(1.0);
    let latest_per_sec = handler.frozen_rate.unwrap_or_else(|| handler.current_rate());
    let suffix = if done { "*" } else { "" };
    let color = palette::handler_color(color_idx);

    let x_labels = vec![Line::from("-30s"), Line::from("-15s"), Line::from("now")];
    let y_labels = vec![
        Line::from("0"),
        Line::from(format!("{:.0}", y_max / 2.0)),
        Line::from(format!("{y_max:.0}")),
    ];

    let dataset = Dataset::default()
        .graph_type(GraphType::Line)
        .style(Style::default().fg(color).add_modifier(Modifier::BOLD))
        .data(data);

    let chart = Chart::new(vec![dataset])
        .block(
            Block::default()
                .title(format!(
                    " {} | {:.1} ev/s{} | handled {} failed {} ",
                    handler.name, latest_per_sec, suffix, handler.total_handled, handler.total_failed
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette::BORDER_ALT))
                .style(Style::default().bg(palette::PANEL_BG)),
        )
        .x_axis(
            Axis::default()
                .title("time")
                .style(Style::default().fg(palette::MUTED))
                .bounds([-WINDOW_SECS, 0.0])
                .labels(x_labels),
        )
        .y_axis(
            Axis::default()
                .title("ev/s")
                .style(Style::default().fg(palette::MUTED))
                .bounds([0.0, y_max])
                .labels(y_labels),
        );

    chart.render(area, buf);
}
