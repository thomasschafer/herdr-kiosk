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
        &state.repo_view.list.input.text,
        state.repo_view.list.input.cursor,
    );

    state.active_list_rows = usize::from(list_area.height.saturating_sub(2)).max(1);
    state
        .repo_view
        .list
        .update_scroll_offset(state.active_list_rows);
    let visible = state.repo_view.list.visible_items(state.active_list_rows);
    let selected = state.repo_view.list.selected.and_then(|selected| {
        visible
            .iter()
            .position(|(position, _)| *position == selected)
    });
    let row_width = usize::from(list_area.width.saturating_sub(4));
    let mut items: Vec<_> = visible
        .iter()
        .filter_map(|(_, index)| state.repo_view.entries.get(*index))
        .map(|entry| {
            let name = crate::display::sanitize(&entry.display_name()).into_owned();
            let left = if entry.repo.is_git {
                vec![Span::raw(name)]
            } else {
                vec![
                    Span::raw(name),
                    Span::styled("  dir", Style::default().fg(theme.muted)),
                ]
            };
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
    if state.repo_view.loading && items.is_empty() {
        items.push(ListItem::new(Span::styled(
            "Discovering repos…",
            Style::default().fg(theme.muted),
        )));
    }

    let scan_suffix = if state.repo_view.loading {
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
                    state.repo_view.list.filtered.len(),
                    state.repo_view.entries.len()
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

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::{Terminal, backend::TestBackend};

    use crate::{
        git::Repo,
        state::{AppState, RepoEntry, SearchableList},
        theme::Theme,
    };

    use super::draw;

    #[test]
    fn repo_controls_render_as_visible_text_on_one_row() {
        let mut state = AppState::new(None);
        state.repo_view.loading = false;
        state.repo_view.entries = vec![RepoEntry::new(Repo {
            name: "repo\nname\u{1b}".into(),
            path: "/repo".into(),
            is_git: true,
            worktrees: Vec::new(),
        })];
        state.repo_view.list = SearchableList::new(1);
        state.repo_view.list.selected = None;
        let theme = Theme::from_config(&crate::config::ThemeConfig::default());
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw(frame, area, &mut state, &theme, Instant::now());
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("repo\\nname\\u{1b}"));
        assert!(!rendered.chars().any(char::is_control));
    }

    #[test]
    fn plain_folders_render_with_a_muted_type_marker() {
        let mut state = AppState::new(None);
        state.repo_view.loading = false;
        state.repo_view.entries = vec![RepoEntry::new(Repo {
            name: "folder".into(),
            path: "/folder".into(),
            is_git: false,
            worktrees: Vec::new(),
        })];
        state.repo_view.list = SearchableList::new(1);
        state.repo_view.list.selected = None;
        let theme = Theme::from_config(&crate::config::ThemeConfig::default());
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw(frame, area, &mut state, &theme, Instant::now());
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("folder  dir"));
    }
}
