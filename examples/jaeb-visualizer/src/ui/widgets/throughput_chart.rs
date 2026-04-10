use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Widget};

use crate::metrics::VisualizationState;

const SAMPLE_SECS: f64 = 0.5;
const WINDOW_SAMPLES: usize = 60;
const WINDOW_SECS: f64 = SAMPLE_SECS * WINDOW_SAMPLES as f64;

/// Renders a dynamic throughput chart (time on X, throughput on Y).
///
/// Newest samples are drawn at the right edge (`x = now`) and older samples move left over time.
pub fn render_throughput_chart(area: Rect, buf: &mut Buffer, state: &VisualizationState) {
    let history_len = state.throughput_history.len();

    let points: Vec<(f64, f64)> = state
        .throughput_history
        .iter()
        .enumerate()
        .map(|(idx, v)| {
            let age_samples = history_len.saturating_sub(1).saturating_sub(idx) as f64;
            let x = -(age_samples * SAMPLE_SECS);
            let y = *v as f64 / SAMPLE_SECS; // convert sample delta (per 500ms) to events/sec
            (x, y)
        })
        .collect();

    let fallback = [(-WINDOW_SECS, 0.0), (0.0, 0.0)];
    let data = if points.is_empty() { &fallback[..] } else { points.as_slice() };

    let peak_per_sec = state
        .throughput_history
        .iter()
        .copied()
        .max()
        .map(|v| v as f64 / SAMPLE_SECS)
        .unwrap_or(1.0)
        .max(1.0);
    let y_max = (peak_per_sec * 1.2).ceil().max(1.0);
    let y_mid = (y_max / 2.0).ceil();
    let latest_per_sec = state.throughput_history.back().map(|v| *v as f64 / SAMPLE_SECS).unwrap_or(0.0);

    let x_labels = vec![
        Line::from(format!("-{:.0}s", WINDOW_SECS)),
        Line::from(format!("-{:.0}s", WINDOW_SECS / 2.0)),
        Line::from("now"),
    ];
    let y_labels = vec![Line::from("0"), Line::from(format!("{y_mid:.0}")), Line::from(format!("{y_max:.0}"))];

    let dataset = Dataset::default()
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .data(data);

    let chart = Chart::new(vec![dataset])
        .block(
            Block::default()
                .title(format!(" Throughput Chart | now: {:.1} ev/s (newest right) ", latest_per_sec))
                .borders(Borders::ALL),
        )
        .x_axis(
            Axis::default()
                .title("time")
                .style(Style::default().fg(Color::DarkGray))
                .bounds([-WINDOW_SECS, 0.0])
                .labels(x_labels),
        )
        .y_axis(
            Axis::default()
                .title("ev/s")
                .style(Style::default().fg(Color::DarkGray))
                .bounds([0.0, y_max])
                .labels(y_labels),
        );

    chart.render(area, buf);
}
