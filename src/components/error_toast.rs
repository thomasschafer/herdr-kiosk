use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
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
    let height = area.height.saturating_sub(2).clamp(1, 7);
    let [horizontal] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [toast_area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::End)
        .areas(horizontal);
    let (label, color) = match toast.kind {
        ToastKind::Warning => ("Warning", theme.warning),
        ToastKind::Error => ("Error", theme.error),
    };
    let counter = queue_counter(state.toasts.len()).map(|counter| format!("  {counter}"));
    let mut body = vec![Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(&toast.message),
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
    use super::{dismiss_hint, queue_counter};
    use crate::config::keys::KeysConfig;

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
}
