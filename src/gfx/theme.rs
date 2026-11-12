//! Colours and metrics shared by all Krab surfaces.

use super::Color;

/// Rust-orange brand accent.
pub const ACCENT: Color = Color::rgb(0xF4, 0x66, 0x23);
pub const ACCENT_HOVER: Color = Color::rgb(0xFF, 0x7A, 0x3A);
pub const ACCENT_DEEP: Color = Color::rgb(0xC8, 0x45, 0x12);

pub const PANEL: Color = Color::rgba(0x1B, 0x1C, 0x20, 0xF7);
pub const PANEL_SOLID: Color = Color::rgb(0x1B, 0x1C, 0x20);
pub const PANEL_BORDER: Color = Color::rgba(255, 255, 255, 30);
pub const HOVER: Color = Color::rgba(255, 255, 255, 26);
pub const PRESSED: Color = Color::rgba(255, 255, 255, 44);
pub const SEPARATOR: Color = Color::rgba(255, 255, 255, 34);

pub const TEXT: Color = Color::rgb(0xF2, 0xF2, 0xF4);
pub const TEXT_DIM: Color = Color::rgb(0xA6, 0xA8, 0xB0);
pub const ICON: Color = Color::rgb(0xE4, 0xE5, 0xE9);
pub const ICON_DISABLED: Color = Color::rgb(0x6A, 0x6C, 0x74);
pub const DANGER: Color = Color::rgb(0xFF, 0x45, 0x3A);

/// Annotation palette (first entry is the default).
pub const PALETTE: [Color; 8] = [
    Color::rgb(0xFF, 0x3B, 0x30), // red
    Color::rgb(0xFF, 0x95, 0x00), // orange
    Color::rgb(0xFF, 0xD6, 0x0A), // yellow
    Color::rgb(0x34, 0xC7, 0x59), // green
    Color::rgb(0x0A, 0x84, 0xFF), // blue
    Color::rgb(0xBF, 0x5A, 0xF2), // purple
    Color::rgb(0xFF, 0xFF, 0xFF), // white
    Color::rgb(0x1C, 0x1C, 0x1E), // black
];

/// Stroke widths (logical px) for the S / M / L size buttons.
pub const WIDTHS: [f32; 3] = [2.5, 4.5, 8.0];

// Metrics in logical pixels; multiply by the monitor scale factor.
pub const BTN: f32 = 34.0;
pub const BTN_GAP: f32 = 2.0;
pub const BAR_PAD: f32 = 5.0;
pub const BAR_RADIUS: f32 = 11.0;
pub const BTN_RADIUS: f32 = 7.0;
pub const ICON_SIZE: f32 = 20.0;
pub const FONT: f32 = 12.5;
