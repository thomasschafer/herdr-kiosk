use std::time::Instant;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Padding, Paragraph,
    },
};

use crate::{
    state::{AppState, Mode},
    theme::Theme,
};

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    theme: &Theme,
    spinner_start: Instant,
) {
    let [horizontal] = Layout::horizontal([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(horizontal);

    let title = match &state.mode {
        Mode::ValidatingNewBranch { name, .. } => {
            format!(" New branch \"{}\" ", crate::display::sanitize(name))
        }
        Mode::SelectBaseBranch { flow, .. } => {
            format!(
                " New branch \"{}\" — pick base ",
                crate::display::sanitize(&flow.new_name)
            )
        }
        _ => return,
    };
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme.secondary))
        .padding(Padding::uniform(1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    match &mut state.mode {
        Mode::ValidatingNewBranch { .. } => {
            let spinner =
                super::repo_list::SPINNER_FOR_LOADING[(spinner_start.elapsed().as_millis() / 80)
                    as usize
                    % super::repo_list::SPINNER_FOR_LOADING.len()];
            frame.render_widget(
                Paragraph::new(vec![
                    Line::raw(""),
                    Line::from(Span::styled(
                        format!("{spinner} Validating branch name…"),
                        Style::default()
                            .fg(theme.secondary)
                            .add_modifier(Modifier::BOLD),
                    )),
                ])
                .alignment(Alignment::Center),
                inner,
            );
        }
        Mode::SelectBaseBranch { flow, .. } => {
            let [search_area, list_area] =
                Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(inner);
            super::search_bar::draw(
                frame,
                search_area,
                &super::search_bar::SearchBarStyle {
                    title: "Base branch",
                    placeholder: "Type to filter local branches…",
                    border_color: theme.secondary,
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
                .map(|base| ListItem::new(Span::raw(crate::display::sanitize(base).into_owned())))
                .collect::<Vec<_>>();
            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {} bases ", flow.list.filtered.len()))
                        .border_style(Style::default().fg(theme.secondary)),
                )
                .highlight_style(
                    Style::default()
                        .bg(theme.secondary)
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
        Mode::RepoSelect
        | Mode::BranchSelect(_)
        | Mode::ConfirmWorktreeDelete(_)
        | Mode::Loading { .. } => {}
    }
}
