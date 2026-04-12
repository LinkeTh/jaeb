use ratatui::style::Color;

pub const BG: Color = Color::Rgb(10, 8, 25);
pub const PANEL_BG: Color = Color::Rgb(18, 14, 42);

pub const BRAND: Color = Color::Rgb(255, 45, 196); // neon magenta
pub const RUNNING: Color = Color::Rgb(66, 255, 245); // neon cyan
pub const DONE: Color = Color::Rgb(255, 241, 102); // neon yellow
pub const ERROR: Color = Color::Rgb(255, 68, 136); // neon pink-red
pub const WARN: Color = Color::Rgb(255, 176, 0); // neon amber
pub const MUTED: Color = Color::Rgb(133, 125, 184);
pub const VALUE: Color = Color::Rgb(192, 255, 255);

pub const BORDER: Color = Color::Rgb(147, 63, 255);
pub const BORDER_ALT: Color = Color::Rgb(0, 210, 255);

pub const HANDLER_COLORS: [Color; 8] = [
    Color::Rgb(0, 255, 255),
    Color::Rgb(255, 0, 255),
    Color::Rgb(255, 255, 0),
    Color::Rgb(0, 255, 128),
    Color::Rgb(255, 102, 0),
    Color::Rgb(102, 153, 255),
    Color::Rgb(255, 51, 153),
    Color::Rgb(51, 255, 204),
];

pub fn handler_color(idx: usize) -> Color {
    HANDLER_COLORS[idx % HANDLER_COLORS.len()]
}
