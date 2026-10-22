use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};

use crate::{
    config::keys::{BindingMode, Command, KeysConfig},
    theme::Theme,
};

pub(crate) fn confirm_cancel_keys(keys: &KeysConfig) -> (Option<String>, Option<String>) {
    (
        keys.first_key(BindingMode::Modal, Command::Open)
            .map(|key| key.to_string()),
        keys.first_key(BindingMode::Modal, Command::Back)
            .map(|key| key.to_string()),
    )
}

pub(crate) fn confirm_cancel_hints(keys: &KeysConfig) -> Vec<(String, &'static str)> {
    let (confirm, cancel) = confirm_cancel_keys(keys);
    confirm
        .map(|key| (key, "confirm"))
        .into_iter()
        .chain(cancel.map(|key| (key, "cancel")))
        .collect()
}

/// Combines the up and down keys into one hint when both are bound.
pub(crate) fn move_hints(keys: &KeysConfig, mode: BindingMode) -> Vec<(String, &'static str)> {
    match (
        keys.preferred_key(mode, Command::MoveUp),
        keys.preferred_key(mode, Command::MoveDown),
    ) {
        (Some(up), Some(down)) => vec![(format!("{up}/{down}"), "move")],
        (Some(up), None) => vec![(up.to_string(), "move up")],
        (None, Some(down)) => vec![(down.to_string(), "move down")],
        (None, None) => Vec::new(),
    }
}

/// Renders `(key, label)` pairs as one line. Returns `None` when there are no hints.
pub(crate) fn hint_line<'a>(hints: &[(String, &'static str)], theme: &Theme) -> Option<Line<'a>> {
    let mut spans = Vec::new();
    for (key, label) in hints {
        if !spans.is_empty() {
            spans.push(Span::raw(" / "));
        }
        spans.push(Span::styled(key.clone(), Style::default().fg(theme.hint)));
        spans.push(Span::raw(format!(" {label}")));
    }
    (!spans.is_empty()).then(|| Line::from(spans))
}

pub struct Dialog<'a> {
    lines: Vec<Line<'a>>,
    border_color: Color,
    title: &'a str,
}

impl<'a> Dialog<'a> {
    pub fn new(title: &'a str, lines: Vec<Line<'a>>, border_color: Color) -> Self {
        Self {
            lines,
            border_color,
            title,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let width = area.width.saturating_sub(4).clamp(20, 100);
        let text_width = width.saturating_sub(6).max(1);
        let content_height = super::wrapped_content_height(&self.lines, text_width);
        let height = content_height.saturating_add(4).min(area.height.max(1));
        let [horizontal] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(area);
        let [dialog_area] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(horizontal);
        frame.render_widget(Clear, dialog_area);
        frame.render_widget(
            Paragraph::new(self.lines.clone())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(self.border_color))
                        .title(self.title)
                        .padding(Padding::uniform(1)),
                )
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false }),
            dialog_area,
        );
    }
}
