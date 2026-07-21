use std::io;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, read};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Layout},
    widgets::Paragraph,
};

pub mod config;
pub mod context;
pub mod event;
pub mod git;
pub mod herdr;
pub mod spawn;
pub mod state;

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
    let loaded_config = config::load_config()?;
    for warning in &loaded_config.warnings {
        eprintln!("herdr-kiosk: warning: {}", warning.message);
    }

    let _restore_guard = TerminalRestoreGuard;
    let mut terminal = ratatui::try_init()?;
    run(&mut terminal)?;
    Ok(())
}

fn run(terminal: &mut DefaultTerminal) -> io::Result<()> {
    loop {
        terminal.draw(draw)?;

        if let Event::Key(key) = read()?
            && should_quit(key)
        {
            return Ok(());
        }
    }
}

fn draw(frame: &mut Frame) {
    let [_, title_area, hint_area, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new("herdr-kiosk").alignment(Alignment::Center),
        title_area,
    );
    frame.render_widget(
        Paragraph::new("q / Esc / Ctrl+C to quit").alignment(Alignment::Center),
        hint_area,
    );
}

fn should_quit(key: KeyEvent) -> bool {
    if key.kind != KeyEventKind::Press {
        return false;
    }

    matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
        || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_keys_exit_on_press() {
        for key in [
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            assert!(should_quit(key), "expected {key:?} to quit");
        }
    }

    #[test]
    fn unrelated_keys_and_key_releases_do_not_exit() {
        assert!(!should_quit(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE
        )));
        assert!(!should_quit(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::CONTROL
        )));
        assert!(!should_quit(KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release
        )));
    }
}
