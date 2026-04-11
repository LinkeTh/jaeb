use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Widget};

use crate::metrics::VisualizationState;
use crate::ui::palette;

/// Renders a channel pressure gauge with units and legend.
pub fn render_pressure(area: Rect, buf: &mut Buffer, state: &VisualizationState) {
    let [gauge_area, legend_area] = Layout::vertical([Constraint::Length(3), Constraint::Length(1)]).areas(area);
    let ratio = state.pressure_ratio();
    let color = if ratio > 0.8 {
        palette::ERROR
    } else if ratio > 0.5 {
        palette::WARN
    } else {
        palette::RUNNING
    };

    let capacity = if state.bus_queue_capacity == 0 {
        state.buffer_size
    } else {
        state.bus_queue_capacity
    };
    let label = format!("{:.0}% buffer used ({}/{})", ratio * 100.0, state.bus_publish_in_flight, capacity);

    let gauge = Gauge::default()
        .block(
            Block::default()
                .title(" Backpressure % (in-flight / buffer) ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette::BORDER))
                .style(Style::default().bg(palette::PANEL_BG)),
        )
        .gauge_style(Style::default().fg(color))
        .ratio(ratio)
        .label(label);

    gauge.render(gauge_area, buf);

    let legend = Line::from(vec![
        Span::styled("  low ", Style::default().fg(palette::RUNNING)),
        Span::styled("->", Style::default().fg(palette::MUTED)),
        Span::styled(" medium ", Style::default().fg(palette::WARN)),
        Span::styled("->", Style::default().fg(palette::MUTED)),
        Span::styled(" high", Style::default().fg(palette::ERROR)),
    ]);
    Paragraph::new(legend).render(legend_area, buf);
}
