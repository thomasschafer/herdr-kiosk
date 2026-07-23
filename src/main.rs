use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use crossterm::event::{self as ct_event, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use herdr_kiosk::{
    app, components,
    config::{self, ConfigWarning},
    context::PluginContext,
    git::{self, CliGitProvider},
    herdr::{CliHerdrProvider, HerdrProvider},
    path,
    setup::{self, SetupState, SetupStep},
    state::{AppState, ToastKind},
    theme::Theme,
};

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
    let mut loaded = config::load_config()?;
    let herdr_binary = std::env::var_os("HERDR_BIN_PATH").filter(|path| !path.is_empty());
    let context = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .filter(|json| !json.is_empty())
        .map(|json| PluginContext::from_json(&json))
        .transpose()
        .map_err(|error| anyhow::anyhow!("invalid HERDR_PLUGIN_CONTEXT_JSON: {error}"))?
        .unwrap_or_default();

    let theme = Theme::from_config(&loaded.config.theme);
    let _restore_guard = TerminalRestoreGuard;
    let mut terminal = ratatui::try_init()?;
    if !loaded.exists
        && let Some(path) = loaded.path.as_ref()
    {
        let Some(search_dirs) = run_setup_wizard(&mut terminal, path, &theme)? else {
            return Ok(());
        };
        loaded.config.search_dirs = search_dirs;
        loaded.exists = true;
    }
    if loaded.config.search_dirs.is_empty() {
        return run_no_search_dirs(&mut terminal, loaded.path.as_ref());
    }

    let home = user_home_dir();
    let resolved = loaded
        .config
        .resolved_search_dirs_with(home.as_deref(), std::path::Path::is_dir)?;
    let mut warnings = loaded.warnings;
    warnings.extend(resolved.warnings);
    let current_cwd = context
        .current_cwd()
        .map(|path| std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)));
    let mut state = AppState::new(current_cwd);
    state.on_open = loaded.config.on_open.clone();
    warnings.extend(herdr_kiosk::screens::delete::load_pending(&mut state));
    for ConfigWarning { message } in warnings {
        state.push_toast(ToastKind::Warning, message);
    }
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
        &loaded.config.keys,
    )?;
    Ok(())
}

fn run_setup_wizard(
    terminal: &mut DefaultTerminal,
    config_path: &std::path::Path,
    theme: &Theme,
) -> Result<Option<Vec<config::SearchDirEntry>>> {
    let mut state = SetupState::default();
    let home = user_home_dir();
    let path_display = herdr_kiosk::display::sanitize(&path::display(config_path)).into_owned();
    loop {
        terminal.draw(|frame| components::setup::draw(frame, &state, theme, &path_display))?;
        let Event::Key(key) = ct_event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(None);
        }
        match &state.step {
            SetupStep::Welcome => match key.code {
                KeyCode::Enter => state.continue_from_welcome(),
                KeyCode::Esc => return Ok(None),
                _ => {}
            },
            SetupStep::Directories => {
                if handle_directory_key(&mut state, key, home.as_deref()) {
                    return Ok(None);
                }
            }
            SetupStep::Depth { .. } => handle_depth_key(&mut state, key, home.as_deref()),
            SetupStep::Confirm => match key.code {
                KeyCode::Enter => {
                    let search_dirs = state.search_dirs();
                    setup::write_config_atomic(config_path, &search_dirs)?;
                    return Ok(Some(search_dirs));
                }
                KeyCode::Esc => {
                    state.step = SetupStep::Directories;
                    state.input.clear();
                }
                _ => {}
            },
        }
    }
}

fn handle_directory_key(
    state: &mut SetupState,
    key: KeyEvent,
    home: Option<&std::path::Path>,
) -> bool {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Enter => {
            if let Err(message) = state.begin_depth() {
                state.message = Some(message.into());
            }
        }
        KeyCode::Tab => state.tab_complete(home),
        KeyCode::Esc if state.input.text.is_empty() => return true,
        KeyCode::Esc => {
            state.input.clear();
            state.update_completions(home);
        }
        KeyCode::Char('x') if control => {
            if state.remove_last().is_none() {
                state.message = Some("No directory to remove".into());
            }
        }
        KeyCode::Backspace if alt => {
            state.input.delete_word();
            state.update_completions(home);
        }
        KeyCode::Backspace => {
            state.input.backspace();
            state.update_completions(home);
        }
        KeyCode::Char('w') if control => {
            state.input.delete_word();
            state.update_completions(home);
        }
        KeyCode::Left if !control && !alt => state.input.cursor_left(),
        KeyCode::Right if !control && !alt => state.input.cursor_right(),
        KeyCode::Char(character) if !control && !alt && !character.is_control() => {
            state.input.insert_char(character);
            state.message = None;
            state.update_completions(home);
        }
        _ => {}
    }
    false
}

fn handle_depth_key(state: &mut SetupState, key: KeyEvent, home: Option<&std::path::Path>) {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Enter => {
            if let Err(message) = state.commit_depth() {
                state.message = Some(message);
            }
        }
        KeyCode::Esc => {
            state.cancel_depth();
            state.update_completions(home);
        }
        KeyCode::Backspace => {
            state.depth_default_pristine = false;
            state.input.backspace();
        }
        KeyCode::Char(character) if character.is_ascii_digit() && !control && !alt => {
            if state.depth_default_pristine {
                state.input.clear();
            }
            state.depth_default_pristine = false;
            state.input.insert_char(character);
        }
        _ => {}
    }
}

fn run_no_search_dirs(terminal: &mut DefaultTerminal, path: Option<&PathBuf>) -> Result<()> {
    let location = path.map_or_else(
        || "No trusted config path could be resolved.".to_string(),
        |path| {
            format!(
                "Config path: {}",
                herdr_kiosk::display::sanitize(&path::display(path))
            )
        },
    );
    loop {
        terminal.draw(|frame| draw_no_search_dirs(frame, &location))?;
        let Event::Key(key) = ct_event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Press
            && (matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
                || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return Ok(());
        }
    }
}

fn user_home_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|value| !value.is_empty());
    #[cfg(windows)]
    let home = home.or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()));
    home.map(PathBuf::from)
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
            Line::raw("q / esc / ctrl+c to quit"),
        ])
        .alignment(Alignment::Center),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_directory_input_accepts_shifted_uppercase_letters() {
        let mut state = SetupState::default();

        handle_directory_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT),
            None,
        );

        assert_eq!(state.input.text, "A");
    }

    #[test]
    fn wizard_directory_input_accepts_unicode() {
        let mut state = SetupState::default();
        for character in ['é', '界'] {
            handle_directory_key(
                &mut state,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
                None,
            );
        }
        assert_eq!(state.input.text, "é界");
    }

    #[test]
    fn wizard_depth_input_accepts_shift_modifier_and_rejects_control() {
        let mut state = SetupState::default();
        state.continue_from_welcome();
        state.input.text = "/Repo".into();
        state.begin_depth().unwrap();

        handle_depth_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::SHIFT),
            None,
        );
        handle_depth_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('3'), KeyModifiers::CONTROL),
            None,
        );

        assert_eq!(state.input.text, "2");
    }

    #[test]
    fn wizard_depth_replaces_the_default_only_on_the_first_edit() {
        for (digits, expected) in [("10", "10"), ("12", "12"), ("2", "2")] {
            let mut state = SetupState::default();
            state.continue_from_welcome();
            state.input.text = "/repo".into();
            state.input.cursor = state.input.text.len();
            state.begin_depth().unwrap();
            for digit in digits.chars() {
                handle_depth_key(
                    &mut state,
                    KeyEvent::new(KeyCode::Char(digit), KeyModifiers::NONE),
                    None,
                );
            }
            assert_eq!(state.input.text, expected);
        }
    }

    #[test]
    fn backspace_removes_the_pristine_depth_default_once() {
        let mut state = SetupState::default();
        state.continue_from_welcome();
        state.input.text = "/repo".into();
        state.input.cursor = state.input.text.len();
        state.begin_depth().unwrap();
        handle_depth_key(
            &mut state,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            None,
        );
        assert!(state.input.text.is_empty());
        assert!(!state.depth_default_pristine);
        handle_depth_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            None,
        );
        assert_eq!(state.input.text, "2");
    }
}
