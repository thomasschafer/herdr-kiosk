use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};

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
