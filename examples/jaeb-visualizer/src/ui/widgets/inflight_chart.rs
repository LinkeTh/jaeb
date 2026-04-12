use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Widget};

use crate::metrics::VisualizationState;
use crate::ui::palette;

const SAMPLE_SECS: f64 = 0.5;
const WINDOW_SAMPLES: usize = 60;
const WINDOW_SECS: f64 = SAMPLE_SECS * WINDOW_SAMPLES as f64;

/// Renders a two-line time-series chart of in-flight handler count and async task count.
pub fn render_inflight(area: Rect, buf: &mut Buffer, state: &VisualizationState) {
    let in_flight_now = state.in_flight.max(0) as usize;
    let async_now = state.bus_in_flight_async;

    let title = format!(" In-flight: {}  Async tasks: {} ", in_flight_now, async_now);

    if area.height < 4 {
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_ALT))
            .style(Style::default().bg(palette::PANEL_BG))
            .render(area, buf);
        return;
    }

    // Build (x, y) point series from oldest → newest.
    // x = negative seconds from now; y = sampled count.
    let in_flight_points: Vec<(f64, f64)> = {
        let len = state.in_flight_history.len();
        state
            .in_flight_history
            .iter()
            .enumerate()
            .map(|(idx, v)| {
                let age = len.saturating_sub(1).saturating_sub(idx) as f64;
                (-(age * SAMPLE_SECS), (*v).max(0) as f64)
            })
            .collect()
    };

    let async_points: Vec<(f64, f64)> = {
        let len = state.async_tasks_history.len();
        state
            .async_tasks_history
            .iter()
            .enumerate()
            .map(|(idx, v)| {
                let age = len.saturating_sub(1).saturating_sub(idx) as f64;
                (-(age * SAMPLE_SECS), *v as f64)
            })
            .collect()
    };

    let fallback: &[(f64, f64)] = &[(-WINDOW_SECS, 0.0), (0.0, 0.0)];
    let in_flight_data: &[(f64, f64)] = if in_flight_points.is_empty() { fallback } else { &in_flight_points };
    let async_data: &[(f64, f64)] = if async_points.is_empty() { fallback } else { &async_points };

    // Shared Y-axis: scale to the max of both series, with a small headroom.
    let peak = in_flight_points
        .iter()
        .chain(async_points.iter())
        .map(|(_, y)| *y)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let y_max = (peak * 1.2).ceil().max(1.0);

    let x_labels = vec![Line::from("-30s"), Line::from("-15s"), Line::from("now")];
    let y_labels = vec![
        Line::from("0"),
        Line::from(format!("{:.0}", y_max / 2.0)),
        Line::from(format!("{y_max:.0}")),
    ];

    let in_flight_dataset = Dataset::default()
        .name("in-flight")
        .graph_type(GraphType::Line)
        .style(Style::default().fg(palette::DONE).add_modifier(Modifier::BOLD))
        .data(in_flight_data);

    let async_dataset = Dataset::default()
        .name("async tasks")
        .graph_type(GraphType::Line)
        .style(Style::default().fg(palette::BRAND).add_modifier(Modifier::BOLD))
        .data(async_data);

    let chart = Chart::new(vec![in_flight_dataset, async_dataset])
        .block(
            Block::default()
                .title(title)
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
                .title("count")
                .style(Style::default().fg(palette::MUTED))
                .bounds([0.0, y_max])
                .labels(y_labels),
        );

    chart.render(area, buf);
}
