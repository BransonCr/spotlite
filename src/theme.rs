//! Colours taken from the kitty theme on this desktop: near-black greys with an amber accent.
//! The background stays the terminal's own, so kitty's opacity shows through.

use ratatui::style::{Color, Modifier, Style};

pub const TEXT: Color = Color::Rgb(0xd9, 0xd8, 0xd2);
pub const BRIGHT: Color = Color::Rgb(0xec, 0xeb, 0xe6);
pub const MUTED: Color = Color::Rgb(0x88, 0x87, 0x80);
pub const FAINT: Color = Color::Rgb(0x5f, 0x5e, 0x59);
pub const LINE: Color = Color::Rgb(0x2e, 0x2e, 0x31);
pub const SELECTED_BG: Color = Color::Rgb(0x23, 0x23, 0x26);
pub const ACCENT: Color = Color::Rgb(0xe3, 0xa5, 0x48);
pub const RED: Color = Color::Rgb(0xd9, 0x67, 0x5e);
pub const GREEN: Color = Color::Rgb(0x94, 0xb4, 0x79);

pub fn text() -> Style {
    Style::new().fg(TEXT)
}

pub fn bright() -> Style {
    Style::new().fg(BRIGHT).add_modifier(Modifier::BOLD)
}

pub fn muted() -> Style {
    Style::new().fg(MUTED)
}

pub fn faint() -> Style {
    Style::new().fg(FAINT)
}

pub fn accent() -> Style {
    Style::new().fg(ACCENT)
}

pub fn border(focused: bool) -> Style {
    Style::new().fg(if focused { ACCENT } else { LINE })
}
