use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};

use crate::{
    config::keys::KeysConfig,
    state::{AppState, ToastKind},
    theme::Theme,
};

fn queue_counter(length: usize) -> Option<String> {
    (length > 1).then(|| format!("1/{length}"))
}

fn dismiss_hint(keys: &KeysConfig) -> Option<String> {
    keys.dismiss_toast_key().map(|key| key.to_string())
}

pub fn draw(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme, keys: &KeysConfig) {
    let Some(toast) = state.toasts.front() else {
        return;
    };
    let width = area.width.saturating_sub(4).clamp(1, 100);
    let (label, color) = match toast.kind {
        ToastKind::Warning => ("Warning", theme.warning),
        ToastKind::Error => ("Error", theme.error),
    };
    let counter = queue_counter(state.toasts.len()).map(|counter| format!("  {counter}"));
    let mut body = vec![Line::from(vec![
        Span::raw(crate::display::sanitize(&toast.message).into_owned()),
        Span::styled(
            counter.unwrap_or_default(),
            Style::default().fg(theme.muted),
        ),
    ])];
    if let Some(hint) = dismiss_hint(keys) {
        body.push(Line::raw(""));
        body.push(Line::from(vec![
            Span::styled(hint, Style::default().fg(theme.hint)),
            Span::raw(" dismiss"),
        ]));
    }
    let text_width = width.saturating_sub(4).max(1);
    let height = super::wrapped_content_height(&body, text_width)
        .saturating_add(2)
        .min(area.height.max(1));
    let [horizontal] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [toast_area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::End)
        .areas(horizontal);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(format!(" {label} "))
        .padding(Padding::horizontal(1));
    frame.render_widget(Clear, toast_area);
    frame.render_widget(
        Paragraph::new(body)
            .block(block)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true }),
        toast_area,
    );
}

#[cfg(test)]
mod tests {
    use ratatui::{
        Terminal,
        backend::TestBackend,
        style::Color,
        text::{Line, Span},
    };

    use super::{dismiss_hint, draw, queue_counter};
    use crate::{
        config::{ThemeConfig, keys::KeysConfig},
        state::{AppState, ToastKind},
        theme::Theme,
    };

    #[test]
    fn queue_counter_only_appears_for_pending_toasts() {
        assert_eq!(queue_counter(0), None);
        assert_eq!(queue_counter(1), None);
        assert_eq!(queue_counter(3).as_deref(), Some("1/3"));
    }

    #[test]
    fn dismiss_hint_uses_the_effective_remapped_binding() {
        let keys = toml::from_str::<KeysConfig>(
            "[general]\n\"C-x\" = \"noop\"\n\"C-d\" = \"dismiss_toast\"",
        )
        .unwrap();
        assert_eq!(dismiss_hint(&keys).as_deref(), Some("ctrl+d"));
    }

    #[test]
    fn toast_label_and_content_height_render_correctly_at_multiple_sizes() {
        let cases = [
            (
                18,
                16,
                ToastKind::Warning,
                "Warning",
                "abcdefghijklmnopqrstuvwxyz0123456789",
                true,
            ),
            (
                52,
                4,
                ToastKind::Error,
                "Error",
                "A toast in a short terminal",
                false,
            ),
            (
                120,
                30,
                ToastKind::Error,
                "Error",
                "A short toast in a wide terminal",
                false,
            ),
        ];

        for (terminal_width, terminal_height, kind, label, message, queued) in cases {
            let mut state = AppState::new(None);
            state.push_toast(kind, message);
            if queued {
                state.push_toast(ToastKind::Error, "queued");
            }
            let keys = KeysConfig::default();
            let theme = Theme::from_config(&ThemeConfig::default());
            let backend = TestBackend::new(terminal_width, terminal_height);
            let mut terminal = Terminal::new(backend).unwrap();

            terminal
                .draw(|frame| {
                    let area = frame.area();
                    draw(frame, area, &state, &theme, &keys);
                })
                .unwrap();

            let buffer = terminal.backend().buffer();
            let rows = buffer
                .content()
                .chunks(usize::from(terminal_width))
                .map(|cells| {
                    cells
                        .iter()
                        .map(ratatui::buffer::Cell::symbol)
                        .collect::<String>()
                })
                .collect::<Vec<_>>();
            let rendered = rows.concat();
            assert_eq!(
                rendered.matches(label).count(),
                1,
                "{terminal_width}x{terminal_height}"
            );
            assert!(!rendered.contains(&format!("{label}:")));

            let top = rows
                .iter()
                .position(|row| row.contains('┌'))
                .expect("toast top border");
            let bottom = rows
                .iter()
                .position(|row| row.contains('└'))
                .expect("toast bottom border");
            let width = terminal_width.saturating_sub(4).clamp(1, 100);
            let text_width = width.saturating_sub(4).max(1);
            let counter = if queued { "  1/2" } else { "" };
            let body = vec![
                Line::from(vec![Span::raw(message), Span::raw(counter)]),
                Line::raw(""),
                Line::raw("ctrl+x dismiss"),
            ];
            let expected_height = super::super::wrapped_content_height(&body, text_width)
                .saturating_add(2)
                .min(terminal_height.max(1));
            assert_eq!(
                u16::try_from(bottom - top + 1).unwrap(),
                expected_height,
                "{terminal_width}x{terminal_height}"
            );
            assert_eq!(bottom, usize::from(terminal_height - 1));
            if queued {
                assert!(rendered.contains("1/2"));
            }

            let expected_color = match kind {
                ToastKind::Warning => theme.warning,
                ToastKind::Error => theme.error,
            };
            assert_label_color(buffer.content(), label, expected_color);
        }
    }

    fn assert_label_color(cells: &[ratatui::buffer::Cell], label: &str, expected: Color) {
        let label = label
            .chars()
            .map(|character| character.to_string())
            .collect::<Vec<_>>();
        let cells = cells
            .windows(label.len())
            .find(|cells| {
                cells
                    .iter()
                    .zip(&label)
                    .all(|(cell, symbol)| cell.symbol() == symbol)
            })
            .expect("toast label cells");
        assert!(cells.iter().all(|cell| cell.fg == expected));
    }
}
