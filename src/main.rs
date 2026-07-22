use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use crossterm::event::{self as ct_event, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    config::ConfigWarning,
    context::PluginContext,
    git::CliGitProvider,
    herdr::{CliHerdrProvider, HerdrProvider},
    state::{AppState, ToastKind},
    theme::Theme,
};

pub mod app;
pub mod components;
pub mod config;
pub mod context;
pub mod event;
pub mod git;
pub mod herdr;
pub mod keyboard;
pub mod keymap;
pub mod pending_delete;
pub mod spawn;
pub mod state;
pub mod theme;

struct TerminalRestoreGuard;

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

fn main() {
    if let Err(error) = start() {
        eprintln!("herdr-kiosk: {error:#}");
        std::process::exit(1);
    }
}

fn start() -> Result<()> {
    let loaded = config::load_config()?;
    let herdr_binary = std::env::var_os("HERDR_BIN_PATH").filter(|path| !path.is_empty());
    let context = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .filter(|json| !json.is_empty())
        .map(|json| PluginContext::from_json(&json))
        .transpose()
        .map_err(|error| anyhow::anyhow!("invalid HERDR_PLUGIN_CONTEXT_JSON: {error}"))?
        .unwrap_or_default();

    let _restore_guard = TerminalRestoreGuard;
    let mut terminal = ratatui::try_init()?;
    if loaded.config.search_dirs.is_empty() {
        return run_no_search_dirs(&mut terminal, loaded.path.as_ref());
    }

    let resolved = loaded.config.resolved_search_dirs_with(
        std::env::var_os("HOME")
            .as_deref()
            .map(std::path::Path::new),
        std::path::Path::is_dir,
    )?;
    let mut warnings = loaded.warnings;
    warnings.extend(resolved.warnings);
    let current_cwd = context
        .current_cwd()
        .map(|path| std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)));
    let mut state = AppState::new(current_cwd);
    state.pending_worktree_deletes = pending_delete::load_pending_worktree_deletes();
    for ConfigWarning { message } in warnings {
        state.push_toast(ToastKind::Warning, message);
    }
    let theme = Theme::from_config(&loaded.config.theme);
    let herdr: Option<Arc<dyn HerdrProvider>> = herdr_binary
        .map(|binary| Arc::new(CliHerdrProvider::new(binary)) as Arc<dyn HerdrProvider>);
    let git = Arc::new(CliGitProvider) as Arc<dyn git::GitProvider>;
    app::run(
        &mut terminal,
        &mut state,
        &git,
        herdr.as_ref(),
        resolved.dirs,
        &theme,
    )?;
    Ok(())
}

fn run_no_search_dirs(terminal: &mut DefaultTerminal, path: Option<&PathBuf>) -> Result<()> {
    let location = path.map_or_else(
        || "No trusted config path could be resolved.".to_string(),
        |path| format!("Config path: {}", path.display()),
    );
    loop {
        terminal.draw(|frame| draw_no_search_dirs(frame, &location))?;
        if let Event::Key(key) = ct_event::read()?
            && key.kind == KeyEventKind::Press
            && (matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
                || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return Ok(());
        }
    }
}

fn draw_no_search_dirs(frame: &mut Frame, location: &str) {
    let [_, area, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Fill(1),
    ])
    .areas(frame.area());
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "No search directories configured",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::raw(""),
            Line::raw(location),
            Line::raw(""),
            Line::raw("q / Esc / Ctrl+C to quit"),
        ])
        .alignment(Alignment::Center),
        area,
    );
}
