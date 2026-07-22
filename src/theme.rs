use ratatui::style::Color;

use std::time::Duration;

#[cfg(unix)]
use std::{
    io::{IsTerminal, Read, Write},
    time::Instant,
};

use crate::config::{ThemeColor, ThemeConfig};

pub struct Theme {
    pub accent: Color,
    pub secondary: Color,
    pub tertiary: Color,
    pub error: Color,
    pub warning: Color,
    pub muted: Color,
    pub border: Color,
    pub hint: Color,
    pub highlight_fg: Color,
    pub open: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTone {
    Light,
    Dark,
}

#[cfg(unix)]
struct RawModeGuard;

#[cfg(unix)]
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

impl Theme {
    pub fn from_config(config: &ThemeConfig) -> Self {
        Self::from_config_with_background(config, None)
    }

    pub fn from_config_with_background(
        config: &ThemeConfig,
        background: Option<BackgroundTone>,
    ) -> Self {
        let defaults = ThemeConfig::default();
        let muted = if background == Some(BackgroundTone::Light) && config.muted == defaults.muted {
            Color::Gray
        } else {
            color(config.muted)
        };
        let border =
            if background == Some(BackgroundTone::Light) && config.border == defaults.border {
                Color::Gray
            } else {
                color(config.border)
            };
        Self {
            accent: color(config.accent),
            secondary: color(config.secondary),
            tertiary: color(config.tertiary),
            error: color(config.error),
            warning: color(config.warning),
            muted,
            border,
            hint: color(config.hint),
            highlight_fg: color(config.highlight_fg),
            open: color(config.open),
        }
    }
}

pub fn parse_osc11_reply(reply: &str) -> Option<(u8, u8, u8)> {
    let payload = reply.strip_prefix("\u{1b}]11;")?;
    let payload = payload
        .strip_suffix('\u{7}')
        .or_else(|| payload.strip_suffix("\u{1b}\\"))?;
    if let Some(hex) = payload.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }
        return Some((
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        ));
    }
    let rgb = payload.strip_prefix("rgb:")?;
    let mut channels = rgb.split('/');
    let parse_channel = |value: &str| {
        if value.is_empty() || value.len() > 4 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let raw = u32::from_str_radix(value, 16).ok()?;
        let max = 16_u32.pow(u32::try_from(value.len()).ok()?) - 1;
        u8::try_from((raw * 255 + max / 2) / max).ok()
    };
    let color = (
        parse_channel(channels.next()?)?,
        parse_channel(channels.next()?)?,
        parse_channel(channels.next()?)?,
    );
    channels.next().is_none().then_some(color)
}

#[cfg(any(unix, test))]
fn osc11_reply_range(bytes: &[u8]) -> Option<std::ops::Range<usize>> {
    const PREFIX: &[u8] = b"\x1b]11;";
    let start = bytes
        .windows(PREFIX.len())
        .position(|window| window == PREFIX)?;
    let payload = start + PREFIX.len();
    let bel = bytes[payload..]
        .iter()
        .position(|byte| *byte == b'\x07')
        .map(|offset| (payload + offset, 1));
    let string_terminator = bytes[payload..]
        .windows(2)
        .position(|window| window == b"\x1b\\")
        .map(|offset| (payload + offset, 2));
    let terminator = match (bel, string_terminator) {
        (Some(left), Some(right)) => Some(if left.0 < right.0 { left } else { right }),
        (Some(terminator), None) | (None, Some(terminator)) => Some(terminator),
        (None, None) => None,
    };
    terminator.map(|(end, terminator_len)| start..end + terminator_len)
}

pub fn infer_background_tone((red, green, blue): (u8, u8, u8)) -> BackgroundTone {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue);
    if luminance > 0.5 {
        BackgroundTone::Light
    } else {
        BackgroundTone::Dark
    }
}

#[cfg(unix)]
pub fn query_background_tone(timeout: Duration) -> Option<BackgroundTone> {
    use std::os::fd::AsRawFd;

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    if !stdin.is_terminal()
        || !stdout.is_terminal()
        || crossterm::terminal::enable_raw_mode().is_err()
    {
        return None;
    }
    let _guard = RawModeGuard;
    if stdout.write_all(b"\x1b]11;?\x07").is_err() || stdout.flush().is_err() {
        return None;
    }

    let start = Instant::now();
    let mut bytes = Vec::with_capacity(64);
    let mut input = stdin.lock();
    while start.elapsed() < timeout && bytes.len() < 256 {
        let remaining = timeout.saturating_sub(start.elapsed());
        let millis = i32::try_from(remaining.as_millis().max(1)).unwrap_or(i32::MAX);
        let mut descriptor = libc::pollfd {
            fd: input.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: descriptor points to one initialized pollfd for the duration of the call.
        if unsafe { libc::poll(&raw mut descriptor, 1, millis) } <= 0 {
            break;
        }
        let mut byte = [0_u8; 1];
        let Ok(count) = input.read(&mut byte) else {
            break;
        };
        if count == 0 {
            break;
        }
        bytes.push(byte[0]);
        // A byte arriving before the terminator is necessarily consumed; reading one at a time
        // leaves every byte after the OSC reply untouched for crossterm.
        if osc11_reply_range(&bytes).is_some() {
            break;
        }
    }
    let range = osc11_reply_range(&bytes)?;
    let reply = std::str::from_utf8(&bytes[range]).ok()?;
    parse_osc11_reply(reply).map(infer_background_tone)
}

#[cfg(not(unix))]
pub fn query_background_tone(_timeout: Duration) -> Option<BackgroundTone> {
    None
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
        assert_eq!(theme.secondary, Color::Cyan);
        assert_eq!(theme.tertiary, Color::Green);
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

    #[test]
    fn parses_common_osc11_reply_forms_and_rejects_malformed_values() {
        assert_eq!(
            parse_osc11_reply("\u{1b}]11;rgb:ffff/8000/0000\u{7}"),
            Some((255, 128, 0))
        );
        assert_eq!(
            parse_osc11_reply("\u{1b}]11;#102030\u{1b}\\"),
            Some((16, 32, 48))
        );
        assert_eq!(parse_osc11_reply("\u{1b}]11;rgb:gg/00/00\u{7}"), None);
        assert_eq!(parse_osc11_reply("unrelated"), None);
    }

    #[test]
    fn osc11_range_stops_before_trailing_input() {
        let input = b"x\x1b]11;rgb:ffff/0000/8000\x07\xe7\x95\x8c";
        let range = osc11_reply_range(input).unwrap();
        assert_eq!(&input[range], b"\x1b]11;rgb:ffff/0000/8000\x07");
        let input = b"\x1b]11;#102030\x1b\\x\x07";
        let range = osc11_reply_range(input).unwrap();
        assert_eq!(&input[range], b"\x1b]11;#102030\x1b\\");
    }

    #[test]
    fn luminance_infers_light_and_dark_backgrounds() {
        assert_eq!(
            infer_background_tone((255, 255, 255)),
            BackgroundTone::Light
        );
        assert_eq!(infer_background_tone((0, 0, 0)), BackgroundTone::Dark);
        assert_eq!(
            infer_background_tone((250, 240, 180)),
            BackgroundTone::Light
        );
        assert_eq!(infer_background_tone((25, 30, 45)), BackgroundTone::Dark);
    }

    #[test]
    fn light_background_refines_default_muted_shades() {
        let theme = Theme::from_config_with_background(
            &ThemeConfig::default(),
            Some(BackgroundTone::Light),
        );
        assert_eq!(theme.muted, Color::Gray);
        assert_eq!(theme.border, Color::Gray);
        assert_eq!(theme.secondary, Color::Cyan);
        assert_eq!(theme.tertiary, Color::Green);
    }

    #[test]
    fn semantic_accents_remain_overridable_terminal_colors() {
        let config = ThemeConfig {
            secondary: ThemeColor::Blue,
            tertiary: ThemeColor::Yellow,
            ..ThemeConfig::default()
        };
        let theme = Theme::from_config_with_background(&config, Some(BackgroundTone::Light));
        assert_eq!(theme.secondary, Color::Blue);
        assert_eq!(theme.tertiary, Color::Yellow);
    }
}
