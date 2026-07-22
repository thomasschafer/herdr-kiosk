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
        SetupStep::Directories => draw_directories(frame, area, state, theme),
        SetupStep::Depth { path } => draw_depth(frame, area, state, theme, path),
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
            Span::raw(path.to_string()),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Enter", Style::default().fg(theme.hint)),
            Span::raw(" continue   "),
            Span::styled("Esc", Style::default().fg(theme.hint)),
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
    let completion_height = u16::try_from(state.completions.len().min(5)).unwrap_or(5);
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(completion_height),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new("Add one or more directories. Enter on an empty input when finished.")
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
        &state.input.text,
        state.input.cursor,
    );
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
            ListItem::new(format!("{marker}{value}"))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(completions).style(Style::default().fg(theme.muted)),
        chunks[2],
    );
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
                    Span::raw(&entry.path),
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
    let message = state
        .message
        .as_deref()
        .unwrap_or("Enter add/finish   Tab complete   Ctrl+X remove last   Esc quit");
    frame.render_widget(
        Paragraph::new(message)
            .style(Style::default().fg(if state.message.is_some() {
                theme.warning
            } else {
                theme.muted
            }))
            .alignment(Alignment::Center),
        chunks[4],
    );
}

fn draw_depth(frame: &mut Frame, area: Rect, state: &SetupState, theme: &Theme, path: &str) {
    let content = centered(64, 12, area);
    let lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("Directory: "),
            Span::styled(
                path.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
        Line::raw("How many directory levels should be scanned?"),
        Line::from(Span::styled(
            "1 is fastest and scans direct children.",
            Style::default().fg(theme.muted),
        )),
        Line::raw(""),
        Line::from(vec![
            Span::raw("Depth: "),
            Span::styled(
                format!("{}▏", state.input.text),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Enter", Style::default().fg(theme.hint)),
            Span::raw(" add directory   "),
            Span::styled("Esc", Style::default().fg(theme.hint)),
            Span::raw(" back"),
        ]),
    ];
    frame.render_widget(Clear, content);
    frame.render_widget(
        Paragraph::new(lines)
            .block(shell("Scan depth", theme))
            .alignment(Alignment::Center),
        content,
    );
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
            Span::raw(format!("{}  (depth {})", entry.path, entry.depth)),
        ]));
    }
    lines.extend([
        Line::raw(""),
        Line::from(vec![
            Span::styled("Write to: ", Style::default().fg(theme.muted)),
            Span::raw(path.to_string()),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Enter", Style::default().fg(theme.hint)),
            Span::raw(" write config and open picker   "),
            Span::styled("Esc", Style::default().fg(theme.hint)),
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
