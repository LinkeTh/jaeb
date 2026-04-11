use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Widget};

use crate::metrics::state::{HandlerViz, VisualizationState};
use crate::ui::palette;

pub fn render_flow(area: Rect, buf: &mut Buffer, state: &VisualizationState) {
    let block = Block::default()
        .title(" Event Flow [publish queue -> handler ingress] ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER_ALT))
        .style(Style::default().bg(palette::PANEL_BG));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.width < 12 || inner.height < 1 {
        return;
    }

    let label_left = "[pub]";
    let label_right = "[hdl]";
    let track_start = inner.x + label_left.len() as u16 + 1;
    let track_end = inner.x + inner.width.saturating_sub(label_right.len() as u16 + 1);
    let track_width = track_end.saturating_sub(track_start).max(1);

    buf.set_string(inner.x, inner.y, label_left, Style::default().fg(palette::DONE));
    buf.set_string(track_end + 1, inner.y, label_right, Style::default().fg(palette::DONE));
    for x in track_start..track_end {
        buf.set_string(x, inner.y, "=", Style::default().fg(palette::TRACK));
    }

    for (i, blip) in state.flow_blips.iter().enumerate() {
        let y = inner.y + (i % inner.height.max(1) as usize) as u16;
        let x = track_start + (blip.progress * track_width as f64).round() as u16;
        let x = x.min(track_end.saturating_sub(1));
        let color = palette::handler_color(blip.event_type_idx);
        buf.set_string(x, y, "*", Style::default().fg(color));
    }
}

pub fn render_handler_lane(area: Rect, buf: &mut Buffer, handler: &HandlerViz, color_idx: usize) {
    let block = Block::default()
        .title(" Flow lane [handler processing timeline] ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER))
        .style(Style::default().bg(palette::PANEL_BG));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.width < 6 || inner.height < 1 {
        return;
    }

    let track_start = inner.x + 1;
    let track_end = inner.x + inner.width.saturating_sub(1);
    let track_width = track_end.saturating_sub(track_start).max(1);
    for x in track_start..track_end {
        buf.set_string(x, inner.y, "=", Style::default().fg(palette::TRACK));
    }

    let base_color = palette::handler_color(color_idx);
    for blip in &handler.flow_blips {
        let x = track_start + (blip.progress * track_width as f64).round() as u16;
        let x = x.min(track_end.saturating_sub(1));
        let (glyph, color) = match blip.success {
            Some(true) => ("+", palette::RUNNING),
            Some(false) => ("x", palette::ERROR),
            None => (">", base_color),
        };
        buf.set_string(x, inner.y, glyph, Style::default().fg(color));
    }
}
