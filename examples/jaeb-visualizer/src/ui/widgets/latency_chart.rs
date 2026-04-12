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

pub fn render_handler_latency(area: Rect, buf: &mut Buffer, handler: &HandlerViz, color_idx: usize, done: bool) {
    let color = palette::handler_color(color_idx);
    let suffix = if done { "*" } else { "" };

    let avg_ms = handler.current_avg_latency_ms();
    let min_str = handler.min_latency_ms.map(|v| format!("{v:.0}ms")).unwrap_or_else(|| "-".into());
    let max_str = if handler.max_latency_ms > 0.0 {
        format!("{:.0}ms", handler.max_latency_ms)
    } else {
        "-".into()
    };

    let title = format!(" avg {avg_ms:.1}ms{suffix}  min {min_str}  max {max_str} ");

    if area.height < 4 {
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_ALT))
            .style(Style::default().bg(palette::PANEL_BG))
            .render(area, buf);
        return;
    }

    let history_len = handler.latency_history.len();

    let points: Vec<(f64, f64)> = handler
        .latency_history
        .iter()
        .enumerate()
        .map(|(idx, v)| {
            let age_samples = history_len.saturating_sub(1).saturating_sub(idx) as f64;
            let x = -(age_samples * SAMPLE_SECS);
            (*v, x) // swap so we can sort by x below
        })
        .map(|(v, x)| (x, v))
        .collect();

    let fallback = [(-WINDOW_SECS, 0.0), (0.0, 0.0)];
    let data = if points.is_empty() { &fallback[..] } else { points.as_slice() };

    let peak_ms = handler.latency_history.iter().copied().fold(0.0_f64, f64::max).max(1.0);
    let y_max = (peak_ms * 1.2).ceil().max(1.0);

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
                .title("ms")
                .style(Style::default().fg(palette::MUTED))
                .bounds([0.0, y_max])
                .labels(y_labels),
        );

    chart.render(area, buf);
}
