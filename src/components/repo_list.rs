use std::time::Instant;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, ListState},
};

use crate::{state::AppState, theme::Theme};

use super::list_row::{right_align_suffix, truncate_spans};

pub const SPINNER_FOR_LOADING: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    theme: &Theme,
    spinner_start: Instant,
) {
    let [search_area, list_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(area);
    super::search_bar::draw(
        frame,
        search_area,
        &super::search_bar::SearchBarStyle {
            title: "herdr-kiosk — select repo",
            placeholder: "Type to search repos…",
            border_color: theme.accent,
            muted_color: theme.muted,
        },
        &state.repo_list.input.text,
        state.repo_list.input.cursor,
    );

    state.active_list_rows = usize::from(list_area.height.saturating_sub(2)).max(1);
    state.repo_list.update_scroll_offset(state.active_list_rows);
    let visible = state.repo_list.visible_items(state.active_list_rows);
    let selected = state.repo_list.selected.and_then(|selected| {
        visible
            .iter()
            .position(|(position, _)| *position == selected)
    });
    let row_width = usize::from(list_area.width.saturating_sub(4));
    let mut items: Vec<_> = visible
        .iter()
        .filter_map(|(_, index)| state.repos.get(*index))
        .map(|entry| {
            let left = [Span::raw(entry.display_name())];
            let spans = if entry.is_open {
                right_align_suffix(
                    &left,
                    &[Span::styled("● open", Style::default().fg(theme.open))],
                    row_width,
                )
            } else {
                truncate_spans(&left, row_width)
            };
            ListItem::new(Line::from(spans))
        })
        .collect();
    if state.loading_repos && items.is_empty() {
        items.push(ListItem::new(Span::styled(
            "Discovering repos…",
            Style::default().fg(theme.muted),
        )));
    }

    let scan_suffix = if state.loading_repos {
        let frame = (spinner_start.elapsed().as_millis() / 80) as usize % SPINNER_FOR_LOADING.len();
        format!(" | scanning… {}", SPINNER_FOR_LOADING[frame])
    } else {
        String::new()
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " {} of {} repos{scan_suffix} ",
                    state.repo_list.filtered.len(),
                    state.repos.len()
                ))
                .border_style(Style::default().fg(theme.border)),
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
    list_state.select(selected);
    frame.render_stateful_widget(list, list_area, &mut list_state);
}
