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

/// Returns `None` when both the confirm and cancel commands are unbound.
pub(crate) fn confirm_cancel_hints<'a>(keys: &KeysConfig, theme: &Theme) -> Option<Line<'a>> {
    let (confirm, cancel) = confirm_cancel_keys(keys);
    let mut hints = Vec::new();
    if let Some(confirm) = confirm {
        hints.push(Span::styled(confirm, Style::default().fg(theme.hint)));
        hints.push(Span::raw(" confirm"));
    }
    if let Some(cancel) = cancel {
        if !hints.is_empty() {
            hints.push(Span::raw(" / "));
        }
        hints.push(Span::styled(cancel, Style::default().fg(theme.hint)));
        hints.push(Span::raw(" cancel"));
    }
    (!hints.is_empty()).then(|| Line::from(hints))
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
