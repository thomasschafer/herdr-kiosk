use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};

use crate::{
    state::{AppState, ToastKind},
    theme::Theme,
};

pub fn draw(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
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
    let body = vec![
        Line::from(vec![
            Span::styled(
                format!("{label}: "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(&toast.message),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Ctrl+X", Style::default().fg(theme.hint)),
            Span::raw(" dismiss"),
        ]),
    ];
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
