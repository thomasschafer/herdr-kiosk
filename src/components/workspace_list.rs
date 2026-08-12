use std::time::Instant;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, HighlightSpacing, List, ListItem, ListState, Paragraph},
};

use crate::{
    config::keys::{BindingMode, Command, KeysConfig},
    screens::workspace::{WorkspaceEntry, WorkspacePickerState},
    theme::Theme,
};

use super::list_row::{right_align_suffix, truncate_spans};

pub fn draw(
    frame: &mut Frame,
    state: &mut WorkspacePickerState,
    theme: &Theme,
    keys: &KeysConfig,
    spinner_start: Instant,
) {
    if state.loading {
        draw_loading(frame, theme, keys, spinner_start);
        return;
    }

    let [main_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    let [search_area, list_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(main_area);
    super::search_bar::draw(
        frame,
        search_area,
        &super::search_bar::SearchBarStyle {
            title: "herdr-kiosk — switch workspace",
            placeholder: "Type to search workspaces…",
            border_color: theme.accent,
            muted_color: theme.muted,
        },
        &state.list.input.text,
        state.list.input.cursor,
    );

    let viewport_rows = usize::from(list_area.height.saturating_sub(2)).max(1);
    state.list.update_scroll_offset(viewport_rows);
    let visible = state.list.visible_items(viewport_rows);
    let selected = state.list.selected.and_then(|selected| {
        visible
            .iter()
            .position(|(position, _)| *position == selected)
    });
    let row_width = usize::from(list_area.width.saturating_sub(4));
    let items: Vec<_> = visible
        .iter()
        .filter_map(|(_, index)| state.entries.get(*index))
        .map(|entry| ListItem::new(Line::from(entry_spans(entry, theme, row_width))))
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " {} of {} workspaces ",
                    state.list.filtered.len(),
                    state.entries.len()
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

    frame.render_widget(
        Paragraph::new(Line::from(footer_spans(state, theme, keys))).alignment(Alignment::Center),
        footer_area,
    );
}

fn entry_spans<'a>(entry: &WorkspaceEntry, theme: &Theme, row_width: usize) -> Vec<Span<'a>> {
    let mut left = vec![Span::raw(
        crate::display::sanitize(&entry.name).into_owned(),
    )];
    if let Some(branch) = &entry.branch {
        left.push(Span::styled(
            format!("  {}", crate::display::sanitize(branch)),
            Style::default().fg(theme.tertiary),
        ));
    }
    if entry.is_current {
        left.push(Span::styled("  current", Style::default().fg(theme.muted)));
    }
    match agent_badge(entry, theme) {
        Some(badge) => right_align_suffix(&left, &[badge], row_width),
        None => truncate_spans(&left, row_width),
    }
}

fn agent_badge<'a>(entry: &WorkspaceEntry, theme: &Theme) -> Option<Span<'a>> {
    let status = entry.agent_status.as_deref()?;
    let color = match status {
        "working" => theme.accent,
        "idle" => theme.open,
        "unknown" => return None,
        _ => theme.muted,
    };
    Some(Span::styled(
        format!("● {}", crate::display::sanitize(status)),
        Style::default().fg(color),
    ))
}

fn footer_spans<'a>(
    state: &WorkspacePickerState,
    theme: &Theme,
    keys: &KeysConfig,
) -> Vec<Span<'a>> {
    if let Some(error) = &state.error {
        return vec![Span::styled(
            crate::display::sanitize(error).into_owned(),
            Style::default().fg(theme.error),
        )];
    }
    let mut hints = Vec::new();
    let mut add = |command, label: &'static str| {
        if let Some(key) = keys.first_key(BindingMode::Repo, command) {
            if !hints.is_empty() {
                hints.push(Span::raw("  "));
            }
            hints.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme.hint),
            ));
            hints.push(Span::raw(format!(" {label}")));
        }
    };
    add(Command::MoveUp, "move");
    add(Command::Open, "switch");
    add(Command::Clear, "clear/quit");
    add(Command::Quit, "quit");
    hints
}

fn draw_loading(frame: &mut Frame, theme: &Theme, keys: &KeysConfig, spinner_start: Instant) {
    let spinner = super::repo_list::SPINNER_FOR_LOADING[(spinner_start.elapsed().as_millis() / 80)
        as usize
        % super::repo_list::SPINNER_FOR_LOADING.len()];
    let [_, area, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(frame.area());
    let mut lines = vec![Line::from(Span::styled(
        format!("{spinner} Loading workspaces…"),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    ))];
    if let Some(key) = keys.first_key(BindingMode::Repo, Command::Quit) {
        lines.push(Line::from(Span::styled(
            format!("{key} to close"),
            Style::default().fg(theme.muted),
        )));
    }
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::{Terminal, backend::TestBackend};

    use crate::{
        config::keys::KeysConfig,
        herdr::{WorkspaceInfo, WorkspaceWorktreeInfo},
        screens::workspace::WorkspacePickerState,
        theme::Theme,
    };

    use super::draw;

    fn render(state: &mut WorkspacePickerState) -> String {
        let theme = Theme::from_config(&crate::config::ThemeConfig::default());
        let keys = KeysConfig::default();
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw(frame, state, &theme, &keys, Instant::now()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn rows_show_repo_name_branch_current_marker_and_agent_badge() {
        let mut state = WorkspacePickerState::new();
        state.load(vec![
            WorkspaceInfo {
                workspace_id: "w_1".into(),
                label: "photodrop fix\u{1b}modals".into(),
                focused: true,
                agent_status: Some("working".into()),
                branch: Some("fix-modals".into()),
                last_focused_unix_ms: Some(200),
                worktree: Some(WorkspaceWorktreeInfo {
                    repo_root: "/repos/photodrop".into(),
                    repo_name: "photodrop".into(),
                }),
            },
            WorkspaceInfo {
                workspace_id: "w_2".into(),
                label: "~".into(),
                focused: false,
                agent_status: Some("idle".into()),
                branch: None,
                last_focused_unix_ms: Some(100),
                worktree: None,
            },
        ]);

        let rendered = render(&mut state);

        assert!(rendered.contains("photodrop  fix-modals  current"));
        assert!(rendered.contains("● working"));
        assert!(rendered.contains("● idle"));
        assert!(rendered.contains("2 of 2 workspaces"));
        assert!(!rendered.chars().any(char::is_control));
    }

    #[test]
    fn errors_replace_the_footer_hints() {
        let mut state = WorkspacePickerState::new();
        state.load(Vec::new());
        state.error = Some("could not focus workspace: gone".into());

        let rendered = render(&mut state);

        assert!(rendered.contains("could not focus workspace: gone"));
    }

    #[test]
    fn loading_state_shows_a_spinner_before_results_arrive() {
        let mut state = WorkspacePickerState::new();

        let rendered = render(&mut state);

        assert!(rendered.contains("Loading workspaces…"));
    }
}
