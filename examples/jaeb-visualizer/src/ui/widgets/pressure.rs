use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Gauge, Widget};

use crate::metrics::VisualizationState;

/// Renders a channel pressure gauge with dynamic coloring.
pub fn render_pressure(area: Rect, buf: &mut Buffer, state: &VisualizationState) {
    let ratio = state.pressure_ratio();
    let color = if ratio > 0.8 {
        Color::Red
    } else if ratio > 0.5 {
        Color::Yellow
    } else {
        Color::Green
    };

    let label = format!("{:.0}%  ({}/{})", ratio * 100.0, state.in_flight.max(0), state.buffer_size);

    let gauge = Gauge::default()
        .block(Block::default().title(" Channel Pressure ").borders(Borders::ALL))
        .gauge_style(Style::default().fg(color))
        .ratio(ratio)
        .label(label);

    gauge.render(area, buf);
}
