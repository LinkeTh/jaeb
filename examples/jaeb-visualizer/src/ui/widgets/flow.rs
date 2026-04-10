use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Widget};

use crate::metrics::VisualizationState;

/// Renders animated event flow dots moving from left (publish) to right (handlers).
pub fn render_flow(area: Rect, buf: &mut Buffer, state: &VisualizationState) {
    let block = Block::default().title(" Event Flow ").borders(Borders::ALL);
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.width < 10 || inner.height < 1 {
        return;
    }

    // Draw the static lane markers
    let label_left = "[pub]";
    let label_right = "[hdl]";
    let track_start = inner.x + label_left.len() as u16 + 1;
    let track_end = inner.x + inner.width.saturating_sub(label_right.len() as u16 + 1);
    let track_width = track_end.saturating_sub(track_start);

    // Draw labels
    buf.set_string(inner.x, inner.y, label_left, Style::default().fg(Color::DarkGray));
    if inner.height > 0 {
        buf.set_string(track_end + 1, inner.y, label_right, Style::default().fg(Color::DarkGray));
    }

    // Draw track line on first row
    for x in track_start..track_end {
        buf.set_string(x, inner.y, "─", Style::default().fg(Color::DarkGray));
    }

    // Draw additional track lines for more rows
    for row in 1..inner.height.min(3) {
        let y = inner.y + row;
        for x in track_start..track_end {
            buf.set_string(x, y, "─", Style::default().fg(Color::DarkGray));
        }
    }

    // Place blips on tracks (round-robin across available rows)
    for (i, blip) in state.flow_blips.iter().enumerate() {
        let row = (i % inner.height.max(1) as usize) as u16;
        let y = inner.y + row.min(inner.height.saturating_sub(1));
        let x = track_start + (blip.progress * track_width as f64).round() as u16;
        let x = x.min(track_end.saturating_sub(1));

        let (ch, color) = if blip.event_type == 0 {
            ("●", Color::Cyan)
        } else {
            ("●", Color::Red)
        };
        buf.set_string(x, y, ch, Style::default().fg(color));
    }
}
