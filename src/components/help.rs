use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph},
};

use crate::{
    config::keys::{BindingMode, KeysConfig},
    state::{HelpBindingRow, HelpOverlayState, SearchableList},
    theme::Theme,
};

enum HelpLayoutEntry {
    Section(&'static str),
    Blank,
    Binding(usize),
}

pub fn overlay(keys: &KeysConfig, mode: BindingMode) -> HelpOverlayState {
    let rows = keys
        .sections(mode)
        .into_iter()
        .flat_map(|section| {
            section
                .bindings
                .into_iter()
                .map(move |(key, command)| HelpBindingRow {
                    section_name: section.name,
                    key_display: key.to_string(),
                    command_name: command.name(),
                    description: command.description(),
                })
        })
        .collect::<Vec<_>>();
    let list = SearchableList::new(rows.len());
    HelpOverlayState { rows, list }
}

fn layout_entries(overlay: &HelpOverlayState) -> Vec<HelpLayoutEntry> {
    let mut entries = Vec::new();
    let mut current_section = None;
    for (row_index, _) in &overlay.list.filtered {
        let Some(row) = overlay.rows.get(*row_index) else {
            continue;
        };
        if current_section != Some(row.section_name) {
            if current_section.is_some() {
                entries.push(HelpLayoutEntry::Blank);
            }
            current_section = Some(row.section_name);
            entries.push(HelpLayoutEntry::Section(row.section_name));
        }
        entries.push(HelpLayoutEntry::Binding(*row_index));
    }
    entries
}

fn visible_items(overlay: &HelpOverlayState, muted: Color) -> (Vec<ListItem<'static>>, Vec<usize>) {
    if overlay.list.filtered.is_empty() {
        return (
            vec![ListItem::new(Span::styled(
                "No matching bindings",
                Style::default().fg(muted).add_modifier(Modifier::ITALIC),
            ))],
            Vec::new(),
        );
    }
    let entries = layout_entries(overlay);
    let mut items = Vec::with_capacity(entries.len());
    let mut binding_item_indices = Vec::with_capacity(overlay.list.filtered.len());
    for entry in entries {
        match entry {
            HelpLayoutEntry::Section(name) => items.push(ListItem::new(Span::styled(
                format!("{name}:"),
                Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ))),
            HelpLayoutEntry::Blank => items.push(ListItem::new("")),
            HelpLayoutEntry::Binding(index) => {
                let row = &overlay.rows[index];
                binding_item_indices.push(items.len());
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("  {:<14}", row.key_display),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(row.description),
                    Span::styled(
                        format!("  ({})", row.command_name),
                        Style::default().fg(muted),
                    ),
                ])));
            }
        }
    }
    (items, binding_item_indices)
}

fn update_scroll(overlay: &mut HelpOverlayState, binding_items: &[usize], viewport_rows: usize) {
    if overlay.list.filtered.is_empty() {
        overlay.list.scroll_offset = 0;
        return;
    }
    let selected = overlay
        .list
        .selected
        .unwrap_or_default()
        .min(overlay.list.filtered.len() - 1);
    let selected_visual = binding_items.get(selected).copied().unwrap_or_default();
    let viewport_rows = viewport_rows.max(1);
    if selected_visual < overlay.list.scroll_offset {
        overlay.list.scroll_offset = selected_visual;
    } else if selected_visual >= overlay.list.scroll_offset.saturating_add(viewport_rows) {
        overlay.list.scroll_offset = selected_visual + 1 - viewport_rows;
    }
    let total_rows = layout_entries(overlay).len();
    overlay.list.scroll_offset = overlay
        .list
        .scroll_offset
        .min(total_rows.saturating_sub(viewport_rows));
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    overlay: &mut HelpOverlayState,
    toast_visible: bool,
    theme: &Theme,
) {
    let width = area.width.saturating_mul(4) / 5;
    let height = area.height.saturating_mul(9) / 10;
    let [horizontal] = Layout::horizontal([Constraint::Length(width.max(1))])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height.max(1))])
        .flex(Flex::Center)
        .areas(horizontal);
    frame.render_widget(Clear, popup);

    let (search_area, notice_area, list_area) = if toast_visible {
        let [search, notice, list] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .areas(popup);
        (search, Some(notice), list)
    } else {
        let [search, list] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(popup);
        (search, None, list)
    };
    super::search_bar::draw(
        frame,
        search_area,
        &super::search_bar::SearchBarStyle {
            title: "Help — active key bindings",
            placeholder: "Filter by key, command, or description…",
            border_color: theme.tertiary,
            muted_color: theme.muted,
        },
        &overlay.list.input.text,
        overlay.list.input.cursor,
    );
    if let Some(notice_area) = notice_area {
        frame.render_widget(
            Paragraph::new(
                "After help closes, notification dismissal takes precedence over mode actions.",
            )
            .style(Style::default().fg(theme.muted))
            .alignment(Alignment::Center),
            notice_area,
        );
    }

    let (items, binding_items) = visible_items(overlay, theme.muted);
    let viewport_rows = usize::from(list_area.height.saturating_sub(2)).max(1);
    update_scroll(overlay, &binding_items, viewport_rows);
    let selected_item = overlay
        .list
        .selected
        .and_then(|selected| binding_items.get(selected))
        .copied();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " {} bindings (esc: close) ",
                    overlay.list.filtered.len()
                ))
                .border_style(Style::default().fg(theme.tertiary)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.tertiary)
                .fg(theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ")
        .highlight_spacing(HighlightSpacing::Always);
    let mut list_state = ListState::default();
    list_state.select(selected_item);
    *list_state.offset_mut() = overlay.list.scroll_offset;
    frame.render_stateful_widget(list, list_area, &mut list_state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_uses_effective_lowercase_bindings_and_search_metadata() {
        let keys = toml::from_str::<KeysConfig>(
            "[branch_select]\n\"C-b\" = \"new_branch\"\n\"C-o\" = \"noop\"",
        )
        .unwrap();
        let overlay = overlay(&keys, BindingMode::Branch);
        let row = overlay
            .rows
            .iter()
            .find(|row| row.command_name == "new_branch")
            .unwrap();
        assert_eq!(row.key_display, "ctrl+b");
        assert!(row.search_text().contains("Create a new branch"));
        assert!(!overlay.rows.iter().any(|row| row.key_display == "ctrl+o"));
    }

    #[test]
    fn filtered_layout_keeps_section_headers_and_separators() {
        let keys = KeysConfig::default();
        let mut overlay = overlay(&keys, BindingMode::Branch);
        let branch = overlay
            .rows
            .iter()
            .position(|row| row.section_name == "branches")
            .unwrap();
        let general = overlay
            .rows
            .iter()
            .position(|row| row.section_name == "general")
            .unwrap();
        overlay.list.filtered = vec![(branch, 1), (general, 1)];
        let entries = layout_entries(&overlay);
        assert!(matches!(entries[0], HelpLayoutEntry::Section("branches")));
        assert!(matches!(entries[2], HelpLayoutEntry::Blank));
        assert!(matches!(entries[3], HelpLayoutEntry::Section("general")));
    }
}
