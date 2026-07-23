use std::time::Instant;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, ListState},
};

use crate::{
    state::{AppState, BranchEntry},
    theme::Theme,
};

use super::{
    list_row::{right_align_suffix, truncate_spans},
    repo_list::SPINNER_FOR_LOADING,
};

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    theme: &Theme,
    spinner_start: Instant,
) {
    let [search_area, list_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(area);
    let repo_name = state
        .branch_context()
        .map_or("?", |context| context.repo_name.as_str());
    let repo_name = crate::display::sanitize(repo_name);
    let title = format!("{repo_name} — select branch");
    super::search_bar::draw(
        frame,
        search_area,
        &super::search_bar::SearchBarStyle {
            title: &title,
            placeholder: "Type to search branches…",
            border_color: theme.secondary,
            muted_color: theme.muted,
        },
        &state.branch_view.list.input.text,
        state.branch_view.list.input.cursor,
    );

    state.active_list_rows = usize::from(list_area.height.saturating_sub(2)).max(1);
    state
        .branch_view
        .list
        .update_scroll_offset(state.active_list_rows);
    let visible = state.branch_view.list.visible_items(state.active_list_rows);
    let selected = state.branch_view.list.selected.and_then(|selected| {
        visible
            .iter()
            .position(|(position, _)| *position == selected)
    });
    let row_width = usize::from(list_area.width.saturating_sub(4));
    let repo_path = state
        .branch_context()
        .map(|context| context.repo_path.clone());
    let mut items: Vec<_> = visible
        .iter()
        .filter_map(|(_, index)| state.branch_view.entries.get(*index))
        .map(|branch| {
            let pinned = repo_path
                .as_deref()
                .is_some_and(|repo_path| state.pins.branch_is_pinned(repo_path, &branch.id()));
            branch_item(branch, pinned, theme, row_width)
        })
        .collect();
    if state.branch_view.loading && items.is_empty() {
        items.push(ListItem::new(Span::styled(
            "Loading branches…",
            Style::default().fg(theme.muted),
        )));
    }

    let loading_suffix = if state.branch_view.loading
        || state.branch_view.fetching_remote_repo.is_some()
    {
        let frame = (spinner_start.elapsed().as_millis() / 80) as usize % SPINNER_FOR_LOADING.len();
        let label = if state.branch_view.loading {
            "loading"
        } else {
            "fetching"
        };
        format!(" | {label}… {}", SPINNER_FOR_LOADING[frame])
    } else {
        String::new()
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " {} of {} branches{loading_suffix} ",
                    state.branch_view.list.filtered.len(),
                    state.branch_view.entries.len()
                ))
                .border_style(Style::default().fg(theme.border)),
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
    list_state.select(selected);
    frame.render_stateful_widget(list, list_area, &mut list_state);
}

fn branch_item(
    branch: &BranchEntry,
    pinned: bool,
    theme: &Theme,
    row_width: usize,
) -> ListItem<'static> {
    let mut left = if branch.remote.is_some() {
        vec![Span::styled(
            crate::display::sanitize(&branch.display_name()).into_owned(),
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        )]
    } else {
        vec![Span::raw(
            crate::display::sanitize(&branch.name).into_owned(),
        )]
    };
    if branch.remote.is_none() && branch.worktree_path.is_some() {
        left.push(Span::styled(
            " (worktree)",
            Style::default().fg(theme.secondary),
        ));
    }
    if branch.remote.is_none() && branch.is_current {
        left.push(Span::styled(" *", Style::default().fg(theme.secondary)));
    }
    if branch.remote.is_none() && branch.is_default {
        left.push(Span::styled(" (default)", Style::default().fg(theme.muted)));
    }
    let mut suffix = Vec::new();
    if pinned {
        suffix.push(Span::styled("◆ pin", Style::default().fg(theme.muted)));
    }
    if branch.open_workspace_id.is_some() {
        if !suffix.is_empty() {
            suffix.push(Span::raw("  "));
        }
        suffix.push(Span::styled("● open", Style::default().fg(theme.open)));
    }
    let spans = if suffix.is_empty() {
        truncate_spans(&left, row_width)
    } else {
        right_align_suffix(&left, &suffix, row_width)
    };
    ListItem::new(Line::from(spans))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::{Terminal, backend::TestBackend, style::Modifier};

    use crate::{
        state::{AppState, BranchContext, BranchEntry, Mode, SearchableList},
        theme::Theme,
    };

    use super::draw;

    #[test]
    fn remote_row_uses_qualified_name_without_redundant_suffix() {
        let mut state = AppState::new(None);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        state.branch_view.entries = BranchEntry::build_remote("origin", &["feature".into()], &[]);
        state.branch_view.list = SearchableList::new(1);
        state.branch_view.list.selected = None;
        let theme = Theme::from_config(&crate::config::ThemeConfig::default());
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                draw(frame, area, &mut state, &theme, Instant::now());
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("origin/feature"));
        assert!(!rendered.contains("(remote)"));
        let label = "origin/feature"
            .chars()
            .map(|character| character.to_string())
            .collect::<Vec<_>>();
        let cells = buffer
            .content()
            .windows(label.len())
            .find(|cells| {
                cells
                    .iter()
                    .zip(&label)
                    .all(|(cell, symbol)| cell.symbol() == symbol)
            })
            .expect("remote label cells");
        assert!(cells.iter().all(|cell| cell.fg == theme.muted));
        assert!(
            cells
                .iter()
                .all(|cell| cell.modifier.contains(Modifier::DIM))
        );
    }

    #[test]
    fn pinned_branch_renders_a_text_marker() {
        let mut state = AppState::new(None);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        state.branch_view.entries = vec![BranchEntry {
            name: "feature".into(),
            worktree_path: None,
            is_current: false,
            is_default: false,
            remote: None,
            open_workspace_id: None,
        }];
        state.branch_view.list = SearchableList::new(1);
        state.branch_view.list.selected = None;
        state.pins.toggle(crate::recency::RecencyKey::branch(
            std::path::Path::new("/repo"),
            crate::state::BranchId::Local("feature".into()),
        ));
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
        assert!(rendered.contains("◆ pin"));
    }
}
