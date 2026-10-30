//! herdr-kiosk's own config sections, read beside kiosk's shared ones.

use anyhow::{Result, bail};
use kiosk_core::config::ConfigExtension;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Direction in which Herdr creates a configured pane.
pub enum OnOpenPaneDirection {
    Right,
    Down,
}

impl OnOpenPaneDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// A command pane created after a new workspace is opened.
pub struct OnOpenPaneConfig {
    /// Shell command Herdr runs in the opened checkout. The command must not be empty.
    pub command: String,
    /// Split direction: `right` or `down`.
    pub direction: OnOpenPaneDirection,
    /// Fraction of the resulting split occupied by the new command pane. The value
    /// must be greater than 0 and less than 1, and defaults to 0.5 when omitted.
    /// The Herdr app always splits in half, so it refuses a pane with a ratio.
    #[serde(default)]
    pub ratio: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// The section is optional and contains no pane definitions by default.
pub struct OnOpenConfig {
    /// Pane definitions, created in order without moving focus from the primary
    /// pane. Commands run from the opened repository or worktree. They run only
    /// when a workspace is newly opened, not when an existing workspace is focused.
    pub panes: Vec<OnOpenPaneConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// In the herdr TUI, the plugin reads `config.toml` from its config directory: run
/// `herdr plugin config-dir thomasschafer.herdr-kiosk` to locate it. In the Herdr
/// app, kiosk reads `$XDG_CONFIG_HOME/herdr-kiosk/config.toml`, which is
/// `~/.config/herdr-kiosk/config.toml` when `XDG_CONFIG_HOME` is unset. All sections are optional, and the defaults are shown below alongside curated
/// examples for settings that benefit from one.
pub struct Config {
    /// Configure command panes created after opening a new workspace.
    pub on_open: OnOpenConfig,
}

impl ConfigExtension for Config {
    fn validate(&self) -> Result<()> {
        for (index, pane) in self.on_open.panes.iter().enumerate() {
            if pane.command.trim().is_empty() {
                bail!("on_open pane {} command must not be empty", index + 1);
            }
            if pane
                .ratio
                .is_some_and(|ratio| !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0)
            {
                bail!(
                    "on_open pane {} ratio must be greater than 0 and less than 1",
                    index + 1
                );
            }
        }
        Ok(())
    }
}

/// The source of this module, for generating the README's config reference.
pub const SOURCE: &str = include_str!("config.rs");

#[cfg(test)]
mod tests {
    use kiosk_core::config::parse_config;

    use super::*;

    fn parse(contents: &str) -> Result<Config> {
        parse_config::<Config>(contents).map(|(_, config, _)| config)
    }

    #[test]
    fn parses_valid_on_open_panes() {
        let (_, config, warnings) = parse_config::<Config>(
            r#"
[on_open]
panes = [
    { command = "hx", direction = "right" },
    { command = "cargo test", direction = "down", ratio = 0.35 },
]
"#,
        )
        .unwrap();

        assert_eq!(warnings, vec![]);
        assert_eq!(
            config.on_open.panes,
            [
                OnOpenPaneConfig {
                    command: "hx".into(),
                    direction: OnOpenPaneDirection::Right,
                    ratio: None,
                },
                OnOpenPaneConfig {
                    command: "cargo test".into(),
                    direction: OnOpenPaneDirection::Down,
                    ratio: Some(0.35),
                },
            ]
        );
    }

    #[test]
    fn rejects_invalid_on_open_panes() {
        let direction = parse(
            r#"[on_open]
panes = [{ command = "hx", direction = "left" }]"#,
        )
        .unwrap_err();
        let message = format!("{direction:#}");
        assert!(message.contains("unknown variant `left`"));
        assert!(message.contains("expected `right` or `down`"));

        let command = parse(
            r#"[on_open]
panes = [{ command = "  ", direction = "right" }]"#,
        )
        .unwrap_err();
        assert!(format!("{command:#}").contains("command must not be empty"));

        for ratio in ["0", "1", "-0.1", "1.1", "nan"] {
            let error = parse(&format!(
                "[on_open]\npanes = [{{ command = \"hx\", direction = \"right\", ratio = {ratio} }}]"
            ))
            .unwrap_err();
            assert!(
                format!("{error:#}").contains("ratio must be greater than 0 and less than 1"),
                "unexpected error for ratio {ratio}: {error:#}"
            );
        }
    }

    #[test]
    fn absent_or_empty_on_open_has_no_panes() {
        assert_eq!(parse("").unwrap().on_open.panes, vec![]);
        assert_eq!(parse("[on_open]").unwrap().on_open.panes, vec![]);
    }

    #[test]
    fn unknown_keys_in_core_and_on_open_sections_are_both_reported() {
        let (_, _, warnings) = parse_config::<Config>(
            "future = 1\n[theme]\nfuture_theme = 1\n[on_open]\nfuture_on_open = 1\n",
        )
        .unwrap();
        let messages: Vec<_> = warnings
            .iter()
            .map(|warning| warning.message.as_str())
            .collect();
        assert_eq!(
            messages,
            [
                "unknown config key ignored: future",
                "unknown config key ignored: theme.future_theme",
                "unknown config key ignored: on_open.future_on_open",
            ]
        );
    }

    #[test]
    fn a_managed_worktree_directory_is_not_a_setting_here() {
        let (_, _, warnings) =
            parse_config::<Config>("[worktrees]\ndirectory = \"~/trees\"\n").unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("worktrees"));
    }
}
