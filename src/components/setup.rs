use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap},
};

use crate::{
    setup::{SetupState, SetupStep},
    theme::Theme,
};

fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let [horizontal] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [vertical] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(horizontal);
    vertical
}

pub fn draw(frame: &mut Frame, state: &SetupState, theme: &Theme, config_path: &str) {
    let area = centered(frame.area().width.saturating_mul(9) / 10, 24, frame.area());
    frame.render_widget(Clear, area);
    match &state.step {
        SetupStep::Welcome => draw_welcome(frame, area, theme, config_path),
        SetupStep::Directories | SetupStep::Depth { .. } => {
            draw_directories(frame, area, state, theme);
        }
        SetupStep::Confirm => draw_confirm(frame, area, state, theme, config_path),
    }
}

fn shell(title: &str, theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(theme.accent))
        .padding(Padding::uniform(1))
}

fn draw_welcome(frame: &mut Frame, area: Rect, theme: &Theme, path: &str) {
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            "Welcome to herdr-kiosk",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::raw("Pick repositories and branches without leaving herdr."),
        Line::raw("This setup will choose the directories to scan for git repositories."),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Config: ", Style::default().fg(theme.muted)),
            Span::raw(crate::display::sanitize(path).into_owned()),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("enter", Style::default().fg(theme.hint)),
            Span::raw(" continue   "),
            Span::styled("esc", Style::default().fg(theme.hint)),
            Span::raw(" quit"),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(shell("Welcome", theme))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_directories(frame: &mut Frame, area: Rect, state: &SetupState, theme: &Theme) {
    let block = shell("Search directories", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let pending_path = match &state.step {
        SetupStep::Depth { path } => Some(path.as_str()),
        _ => None,
    };
    let auxiliary_height = pending_path.map_or_else(
        || u16::try_from(state.completions.len().min(5)).unwrap_or(5),
        |_| 3,
    );
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(auxiliary_height),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new("Add one or more directories. Submit an empty input when finished.")
            .alignment(Alignment::Center),
        chunks[0],
    );
    super::search_bar::draw(
        frame,
        chunks[1],
        &super::search_bar::SearchBarStyle {
            title: "Directory path",
            placeholder: "~/Development",
            border_color: theme.border,
            muted_color: theme.muted,
        },
        pending_path.unwrap_or(&state.input.text),
        pending_path.map_or(state.input.cursor, str::len),
    );
    draw_directory_auxiliary(frame, chunks[2], state, theme, pending_path);
    let dirs = if state.dirs.is_empty() {
        vec![ListItem::new(Span::styled(
            "No directories added yet",
            Style::default().fg(theme.muted),
        ))]
    } else {
        state
            .dirs
            .iter()
            .map(|entry| {
                ListItem::new(Line::from(vec![
                    Span::styled("✓ ", Style::default().fg(theme.open)),
                    Span::raw(crate::display::sanitize(&entry.path).into_owned()),
                    Span::styled(
                        format!("  depth {}", entry.depth),
                        Style::default().fg(theme.muted),
                    ),
                ]))
            })
            .collect()
    };
    frame.render_widget(
        List::new(dirs).block(Block::default().title(" Added ").borders(Borders::TOP)),
        chunks[3],
    );
    let default_message = if pending_path.is_some() {
        "enter add directory   esc edit path"
    } else {
        "enter add/finish   tab complete   ctrl+x remove last   esc quit"
    };
    let message = state.message.as_deref().unwrap_or(default_message);
    frame.render_widget(
        Paragraph::new(crate::display::sanitize(message))
            .style(Style::default().fg(if state.message.is_some() {
                theme.warning
            } else {
                theme.muted
            }))
            .alignment(Alignment::Center),
        chunks[4],
    );
}

fn draw_directory_auxiliary(
    frame: &mut Frame,
    area: Rect,
    state: &SetupState,
    theme: &Theme,
    pending_path: Option<&str>,
) {
    if pending_path.is_some() {
        super::search_bar::draw(
            frame,
            area,
            &super::search_bar::SearchBarStyle {
                title: "Depth",
                placeholder: "1",
                border_color: theme.accent,
                muted_color: theme.muted,
            },
            &state.input.text,
            state.input.cursor,
        );
    } else {
        let completions = state
            .completions
            .iter()
            .take(5)
            .enumerate()
            .map(|(index, value)| {
                let marker = if state.selected_completion == Some(index) {
                    "▸ "
                } else {
                    "  "
                };
                ListItem::new(format!("{marker}{}", crate::display::sanitize(value)))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(completions).style(Style::default().fg(theme.muted)),
            area,
        );
    }
}

fn draw_confirm(frame: &mut Frame, area: Rect, state: &SetupState, theme: &Theme, path: &str) {
    let mut lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            "Ready to start",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];
    for entry in &state.dirs {
        lines.push(Line::from(vec![
            Span::styled("✓ ", Style::default().fg(theme.open)),
            Span::raw(format!(
                "{}  (depth {})",
                crate::display::sanitize(&entry.path),
                entry.depth
            )),
        ]));
    }
    lines.extend([
        Line::raw(""),
        Line::from(vec![
            Span::styled("Write to: ", Style::default().fg(theme.muted)),
            Span::raw(crate::display::sanitize(path).into_owned()),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("enter", Style::default().fg(theme.hint)),
            Span::raw(" write config and open picker   "),
            Span::styled("esc", Style::default().fg(theme.hint)),
            Span::raw(" back"),
        ]),
    ]);
    frame.render_widget(
        Paragraph::new(lines)
            .block(shell("Confirm setup", theme))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        area,
    );
}
