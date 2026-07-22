use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState},
};

use crate::{
    state::{AppState, Mode},
    theme::Theme,
};

pub fn draw(frame: &mut Frame, area: Rect, state: &mut AppState, theme: &Theme) {
    let Mode::SelectBaseBranch { flow, .. } = &mut state.mode else {
        return;
    };
    let [horizontal] = Layout::horizontal([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(horizontal);
    frame.render_widget(Clear, popup);
    let [search_area, list_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(popup);
    let title = format!("New branch \"{}\" — pick base", flow.new_name);
    super::search_bar::draw(
        frame,
        search_area,
        &super::search_bar::SearchBarStyle {
            title: &title,
            placeholder: "Select base branch…",
            border_color: theme.accent,
            muted_color: theme.muted,
        },
        &flow.list.input.text,
        flow.list.input.cursor,
    );
    let items = flow
        .list
        .filtered
        .iter()
        .filter_map(|(index, _)| flow.bases.get(*index))
        .map(|base| ListItem::new(Span::raw(base.clone())))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.accent)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.accent)
                .fg(theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ")
        .highlight_spacing(HighlightSpacing::Always);
    let mut list_state = ListState::default();
    list_state.select(flow.list.selected);
    flow.list
        .update_scroll_offset(usize::from(list_area.height.saturating_sub(2)).max(1));
    *list_state.offset_mut() = flow.list.scroll_offset;
    frame.render_stateful_widget(list, list_area, &mut list_state);
}
