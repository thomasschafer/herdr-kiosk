use ratatui::style::Color;

use crate::config::{ThemeColor, ThemeConfig};

pub struct Theme {
    pub accent: Color,
    pub error: Color,
    pub warning: Color,
    pub muted: Color,
    pub border: Color,
    pub hint: Color,
    pub highlight_fg: Color,
    pub open: Color,
}

impl Theme {
    pub fn from_config(config: &ThemeConfig) -> Self {
        Self {
            accent: color(config.accent),
            error: color(config.error),
            warning: color(config.warning),
            muted: color(config.muted),
            border: color(config.border),
            hint: color(config.hint),
            highlight_fg: color(config.highlight_fg),
            open: color(config.open),
        }
    }
}

fn color(color: ThemeColor) -> Color {
    match color {
        ThemeColor::Black => Color::Black,
        ThemeColor::Red => Color::Red,
        ThemeColor::Green => Color::Green,
        ThemeColor::Yellow => Color::Yellow,
        ThemeColor::Blue => Color::Blue,
        ThemeColor::Magenta => Color::Magenta,
        ThemeColor::Cyan => Color::Cyan,
        ThemeColor::White => Color::White,
        ThemeColor::Gray => Color::Gray,
        ThemeColor::DarkGray => Color::DarkGray,
        ThemeColor::Reset => Color::Reset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_is_limited_to_terminal_palette_colors() {
        let theme = Theme::from_config(&ThemeConfig::default());
        assert_eq!(theme.accent, Color::Magenta);
        assert_eq!(theme.open, Color::Green);
        assert!(matches!(
            theme.accent,
            Color::Reset
                | Color::Black
                | Color::Red
                | Color::Green
                | Color::Yellow
                | Color::Blue
                | Color::Magenta
                | Color::Cyan
                | Color::Gray
                | Color::DarkGray
                | Color::White
        ));
    }
}
