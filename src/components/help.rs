use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};

use crate::{
    config::keys::{BindingMode, KeysConfig},
    theme::Theme,
};

pub fn help_lines(keys: &KeysConfig, mode: BindingMode, toast_visible: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if toast_visible && let Some(key) = keys.dismiss_toast_key() {
        lines.push(Line::from(vec![
            Span::styled(
                "Toast visible: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                key.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" dismisses the toast before any mode action on the same key."),
        ]));
        lines.push(Line::raw(""));
    }
    for section in keys.sections(mode) {
        lines.push(Line::from(Span::styled(
            format!("{}:", section.name),
            Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        for (key, command) in section.bindings {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<14}", key.to_string()),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(command.description()),
            ]));
        }
        lines.push(Line::raw(""));
    }
    lines.push(Line::from(Span::styled(
        "Esc closes help and returns to the picker",
        Style::default().add_modifier(Modifier::ITALIC),
    )));
    lines
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    keys: &KeysConfig,
    mode: BindingMode,
    toast_visible: bool,
    theme: &Theme,
) {
    let width = area.width.saturating_mul(4) / 5;
    let height = area.height.saturating_mul(9) / 10;
    let [horizontal] = Layout::horizontal([Constraint::Length(width.max(1))])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height.max(1))])
        .flex(Flex::Center)
        .areas(horizontal);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help — active key bindings ")
        .border_style(Style::default().fg(theme.accent))
        .padding(Padding::uniform(1));
    frame.render_widget(
        Paragraph::new(help_lines(keys, mode, toast_visible))
            .block(block)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false }),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_uses_overridden_binding_and_documents_toast_precedence() {
        let keys = toml::from_str::<KeysConfig>(
            "[branch_select]\n\"C-b\" = \"new_branch\"\n\"C-o\" = \"noop\"",
        )
        .unwrap();
        let text = help_lines(&keys, BindingMode::Branch, true)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Ctrl+B"));
        assert!(!text.contains("Ctrl+O"));
        assert!(text.contains("dismisses the toast before"));
    }
}
