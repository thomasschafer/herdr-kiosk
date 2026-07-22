use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub mod keys;
pub use keys::KeysConfig;

pub const APP_NAME: &str = "herdr-kiosk";
pub const DEFAULT_SEARCH_DEPTH: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SearchDirEntry {
    Simple(String),
    Rich { path: String, depth: Option<u16> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    Gray,
    DarkGray,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub accent: ThemeColor,
    pub secondary: ThemeColor,
    pub tertiary: ThemeColor,
    pub error: ThemeColor,
    pub warning: ThemeColor,
    pub muted: ThemeColor,
    pub border: ThemeColor,
    pub hint: ThemeColor,
    pub highlight_fg: ThemeColor,
    pub open: ThemeColor,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            accent: ThemeColor::Magenta,
            secondary: ThemeColor::Cyan,
            tertiary: ThemeColor::Green,
            error: ThemeColor::Red,
            warning: ThemeColor::Yellow,
            muted: ThemeColor::DarkGray,
            border: ThemeColor::DarkGray,
            hint: ThemeColor::Blue,
            highlight_fg: ThemeColor::Black,
            open: ThemeColor::Green,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnOpenPaneDirection {
    Left,
    Right,
    Up,
    Down,
}

impl OnOpenPaneDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OnOpenPaneConfig {
    pub command: String,
    pub direction: OnOpenPaneDirection,
    #[serde(default)]
    pub ratio: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OnOpenConfig {
    pub panes: Vec<OnOpenPaneConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub search_dirs: Vec<SearchDirEntry>,
    pub keys: KeysConfig,
    pub theme: ThemeConfig,
    pub on_open: OnOpenConfig,
}

impl Config {
    pub fn resolved_search_dirs_with(
        &self,
        home: Option<&Path>,
        is_dir: impl Fn(&Path) -> bool,
    ) -> Result<ResolvedSearchDirs> {
        let expanded = self
            .search_dirs
            .iter()
            .map(|entry| {
                let (path, depth) = match entry {
                    SearchDirEntry::Simple(path) => (path, DEFAULT_SEARCH_DEPTH),
                    SearchDirEntry::Rich { path, depth } => {
                        (path, depth.unwrap_or(DEFAULT_SEARCH_DEPTH))
                    }
                };
                expand_tilde(path, home).map(|path| (path, depth))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut dirs = Vec::with_capacity(expanded.len());
        let mut warnings = Vec::new();
        for (path, depth) in expanded {
            if is_dir(&path) {
                dirs.push((path, depth));
            } else {
                warnings.push(ConfigWarning {
                    message: format!(
                        "configured search directory does not exist or is not a directory: {}",
                        path.display()
                    ),
                });
            }
        }
        Ok(ResolvedSearchDirs { dirs, warnings })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWarning {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSearchDirs {
    pub dirs: Vec<(PathBuf, u16)>,
    pub warnings: Vec<ConfigWarning>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig {
    pub config: Config,
    pub path: Option<PathBuf>,
    pub exists: bool,
    pub warnings: Vec<ConfigWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPathResolution {
    pub path: Option<PathBuf>,
    pub warnings: Vec<ConfigWarning>,
}

pub fn resolve_config_path(get_env: impl Fn(&str) -> Option<String>) -> ConfigPathResolution {
    let candidates = [
        get_env("HERDR_PLUGIN_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .map(|value| ("HERDR_PLUGIN_CONFIG_DIR", PathBuf::from(value), false)),
        get_env("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(|value| ("XDG_CONFIG_HOME", PathBuf::from(value), true)),
        get_env("HOME")
            .filter(|value| !value.is_empty())
            .map(|value| ("HOME", PathBuf::from(value).join(".config"), true)),
    ];
    let (path, warnings) =
        resolve_trusted_file_path(candidates.into_iter().flatten(), "config.toml", "config");
    ConfigPathResolution { path, warnings }
}

pub(crate) fn resolve_trusted_file_path(
    candidates: impl IntoIterator<Item = (&'static str, PathBuf, bool)>,
    file_name: &str,
    directory_kind: &str,
) -> (Option<PathBuf>, Vec<ConfigWarning>) {
    let mut warnings = Vec::new();
    for (source, base, add_app_dir) in candidates {
        if !base.is_absolute() {
            // A cwd-relative fallback lets a browsed repository supply plugin
            // files, so only absolute environment-derived locations are trusted.
            warnings.push(ConfigWarning {
                message: format!(
                    "refusing relative {directory_kind} directory from {source}: {}",
                    base.display()
                ),
            });
            continue;
        }
        let path = if add_app_dir {
            base.join(APP_NAME).join(file_name)
        } else {
            base.join(file_name)
        };
        return (Some(path), warnings);
    }
    (None, warnings)
}

pub fn parse_config(contents: &str) -> Result<(Config, Vec<ConfigWarning>)> {
    let mut unknown_keys = Vec::new();
    let deserializer = toml::Deserializer::parse(contents).context("invalid config TOML")?;
    let config: Config = serde_ignored::deserialize(deserializer, |path| {
        unknown_keys.push(path.to_string());
    })
    .context("invalid config TOML")?;
    validate_config(&config)?;
    let warnings = unknown_keys
        .into_iter()
        .map(|path| ConfigWarning {
            message: format!("unknown config key ignored: {path}"),
        })
        .collect();
    Ok((config, warnings))
}

pub fn load_config_with(
    get_env: impl Fn(&str) -> Option<String>,
    read_file: impl Fn(&Path) -> io::Result<Option<String>>,
) -> Result<LoadedConfig> {
    let resolution = resolve_config_path(get_env);
    let Some(path) = resolution.path else {
        return Ok(LoadedConfig {
            config: Config::default(),
            path: None,
            exists: false,
            warnings: resolution.warnings,
        });
    };
    let Some(contents) =
        read_file(&path).with_context(|| format!("failed to read config {}", path.display()))?
    else {
        return Ok(LoadedConfig {
            config: Config::default(),
            path: Some(path),
            exists: false,
            warnings: resolution.warnings,
        });
    };

    let (config, mut parse_warnings) =
        parse_config(&contents).with_context(|| format!("in {}", path.display()))?;
    let mut warnings = resolution.warnings;
    warnings.append(&mut parse_warnings);
    Ok(LoadedConfig {
        config,
        path: Some(path),
        exists: true,
        warnings,
    })
}

pub fn load_config() -> Result<LoadedConfig> {
    load_config_with(
        |name| std::env::var(name).ok(),
        |path| match fs::read_to_string(path) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        },
    )
}

fn validate_config(config: &Config) -> Result<()> {
    for entry in &config.search_dirs {
        let (path, depth) = match entry {
            SearchDirEntry::Simple(path) => (path, DEFAULT_SEARCH_DEPTH),
            SearchDirEntry::Rich { path, depth } => (path, depth.unwrap_or(DEFAULT_SEARCH_DEPTH)),
        };
        if path.trim().is_empty() {
            bail!("search directory path must not be empty");
        }
        if depth == 0 {
            bail!("search directory depth must be at least 1 for {path}");
        }
    }
    for (index, pane) in config.on_open.panes.iter().enumerate() {
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

fn expand_tilde(path: &str, home: Option<&Path>) -> Result<PathBuf> {
    if path == "~" {
        return home
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("cannot expand '~' because HOME is unavailable"));
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        return home
            .map(|home| home.join(rest))
            .ok_or_else(|| anyhow::anyhow!("cannot expand '{path}' because HOME is unavailable"));
    }
    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn absolute_test_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join("herdr-kiosk-config-tests")
            .join(name)
    }

    fn path_string(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn parses_simple_and_depth_search_directory_forms() {
        let (config, warnings) = parse_config(
            r#"search_dirs = [
                "~/Development",
                { path = "~/Work", depth = 3 },
                { path = "~/Projects" }
            ]"#,
        )
        .unwrap();
        assert!(warnings.is_empty());
        assert_eq!(
            config.search_dirs,
            [
                SearchDirEntry::Simple("~/Development".into()),
                SearchDirEntry::Rich {
                    path: "~/Work".into(),
                    depth: Some(3),
                },
                SearchDirEntry::Rich {
                    path: "~/Projects".into(),
                    depth: None,
                },
            ]
        );
    }

    #[test]
    fn parses_valid_on_open_panes() {
        let (config, warnings) = parse_config(
            r#"
[on_open]
panes = [
    { command = "hx", direction = "right" },
    { command = "cargo test", direction = "down", ratio = 0.35 },
]
"#,
        )
        .unwrap();

        assert!(warnings.is_empty());
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
        let direction = parse_config(
            r#"[on_open]
panes = [{ command = "hx", direction = "diagonal" }]"#,
        )
        .unwrap_err();
        assert!(format!("{direction:#}").contains("unknown variant `diagonal`"));

        let command = parse_config(
            r#"[on_open]
panes = [{ command = "  ", direction = "right" }]"#,
        )
        .unwrap_err();
        assert!(format!("{command:#}").contains("command must not be empty"));

        for ratio in ["0", "1", "-0.1", "1.1", "nan"] {
            let error = parse_config(&format!(
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
        assert!(parse_config("").unwrap().0.on_open.panes.is_empty());
        assert!(
            parse_config("[on_open]")
                .unwrap()
                .0
                .on_open
                .panes
                .is_empty()
        );
    }

    #[test]
    fn expands_tilde_with_injected_home_and_preserves_depth() {
        let (config, _) =
            parse_config(r#"search_dirs = ["~", { path = "~/Work", depth = 4 }, "/absolute"]"#)
                .unwrap();
        let resolved = config
            .resolved_search_dirs_with(Some(Path::new("/home/tester")), |_| true)
            .unwrap();
        assert_eq!(
            resolved.dirs,
            [
                (PathBuf::from("/home/tester"), 1),
                (PathBuf::from("/home/tester/Work"), 4),
                (PathBuf::from("/absolute"), 1),
            ]
        );
        assert!(resolved.warnings.is_empty());
    }

    #[test]
    fn missing_home_makes_tilde_expansion_fail_loudly() {
        let (config, _) = parse_config(r#"search_dirs = ["~/Work"]"#).unwrap();
        assert!(config.resolved_search_dirs_with(None, |_| true).is_err());
    }

    #[test]
    fn missing_search_directories_are_reported_instead_of_silently_dropped() {
        let (config, _) = parse_config(r#"search_dirs = ["/exists", "/missing"]"#).unwrap();
        let resolved = config
            .resolved_search_dirs_with(None, |path| path == Path::new("/exists"))
            .unwrap();

        assert_eq!(resolved.dirs, [(PathBuf::from("/exists"), 1)]);
        assert_eq!(resolved.warnings.len(), 1);
        assert!(resolved.warnings[0].message.contains("/missing"));
    }

    #[test]
    fn config_path_uses_documented_precedence() {
        let plugin = absolute_test_path("plugin-config");
        let xdg = absolute_test_path("xdg");
        let home = absolute_test_path("home");
        let values = HashMap::from([
            ("HERDR_PLUGIN_CONFIG_DIR", path_string(&plugin)),
            ("XDG_CONFIG_HOME", path_string(&xdg)),
            ("HOME", path_string(&home)),
        ]);
        let resolution = resolve_config_path(|name| values.get(name).cloned());
        assert_eq!(resolution.path, Some(plugin.join("config.toml")));
    }

    #[test]
    fn relative_config_fallbacks_are_refused() {
        let values = HashMap::from([
            ("HERDR_PLUGIN_CONFIG_DIR", "plugin-config"),
            ("XDG_CONFIG_HOME", ".config"),
            ("HOME", "relative-home"),
        ]);
        let resolution = resolve_config_path(|name| values.get(name).map(ToString::to_string));
        assert!(resolution.path.is_none());
        assert_eq!(resolution.warnings.len(), 3);
        assert!(
            resolution
                .warnings
                .iter()
                .all(|warning| warning.message.contains("refusing relative"))
        );
    }

    #[test]
    fn relative_higher_priority_value_falls_through_to_absolute_xdg() {
        let xdg = absolute_test_path("xdg-fallback");
        let values = HashMap::from([
            ("HERDR_PLUGIN_CONFIG_DIR", "plugin-config".to_string()),
            ("XDG_CONFIG_HOME", path_string(&xdg)),
        ]);
        let resolution = resolve_config_path(|name| values.get(name).cloned());
        assert_eq!(resolution.path, Some(xdg.join("herdr-kiosk/config.toml")));
        assert_eq!(resolution.warnings.len(), 1);
    }

    #[test]
    fn unknown_keys_are_ignored_with_warnings() {
        let (config, warnings) = parse_config(
            r#"
search_dirs = []
future_root_key = true

[theme]
future_theme_key = "blue"
"#,
        )
        .unwrap();
        assert!(config.search_dirs.is_empty());
        assert_eq!(warnings.len(), 2);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.message.contains("future_root_key"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.message.contains("future_theme_key"))
        );
    }

    #[test]
    fn theme_accepts_named_terminal_colors_and_rejects_truecolor_values() {
        let (config, warnings) = parse_config(
            r#"
[theme]
accent = "cyan"
secondary = "blue"
tertiary = "yellow"
highlight_fg = "reset"
"#,
        )
        .unwrap();
        assert!(warnings.is_empty());
        assert_eq!(config.theme.accent, ThemeColor::Cyan);
        assert_eq!(config.theme.secondary, ThemeColor::Blue);
        assert_eq!(config.theme.tertiary, ThemeColor::Yellow);
        assert_eq!(config.theme.highlight_fg, ThemeColor::Reset);
        assert!(parse_config("[theme]\naccent = \"#ff00ff\"").is_err());
        assert!(parse_config("[theme]\nsecondary = \"#00ffff\"").is_err());
        assert!(parse_config("[theme]\ntertiary = [0, 255, 0]").is_err());
        assert!(parse_config("[theme]\naccent = [255, 0, 255]").is_err());
    }

    #[test]
    fn missing_config_loads_empty_defaults_without_reading_relative_paths() {
        let loaded = load_config_with(|_| None, |_| panic!("no path should be read")).unwrap();
        assert!(loaded.config.search_dirs.is_empty());
        assert!(loaded.path.is_none());
        assert!(!loaded.exists);
    }

    #[test]
    fn injected_reader_loads_config() {
        let home = absolute_test_path("injected-home");
        let expected = home.join(".config/herdr-kiosk/config.toml");
        let loaded = load_config_with(
            |name| (name == "HOME").then(|| path_string(&home)),
            |path| {
                assert_eq!(path, expected);
                Ok(Some("search_dirs = [\"/repos\"]".into()))
            },
        )
        .unwrap();
        assert_eq!(loaded.config.search_dirs.len(), 1);
        assert!(loaded.exists);
    }

    #[test]
    fn resolved_but_missing_and_existing_empty_configs_are_distinguished() {
        let config_dir = absolute_test_path("resolved-config");
        let missing = load_config_with(
            |name| (name == "HERDR_PLUGIN_CONFIG_DIR").then(|| path_string(&config_dir)),
            |_| Ok(None),
        )
        .unwrap();
        assert_eq!(
            missing.path.as_deref(),
            Some(config_dir.join("config.toml").as_path())
        );
        assert!(!missing.exists);

        let empty = load_config_with(
            |name| (name == "HERDR_PLUGIN_CONFIG_DIR").then(|| path_string(&config_dir)),
            |_| Ok(Some("search_dirs = []".into())),
        )
        .unwrap();
        assert!(empty.exists);
        assert!(empty.config.search_dirs.is_empty());
    }

    #[test]
    fn invalid_key_chords_and_unknown_actions_are_config_errors() {
        let chord = parse_config("[keys.branch_select]\n\"Hyper-B\" = \"new_branch\"").unwrap_err();
        assert!(format!("{chord:#}").contains("invalid key chord"));
        let action =
            parse_config("[keys.branch_select]\n\"C-b\" = \"unknown_action\"").unwrap_err();
        assert!(format!("{action:#}").contains("unknown key action"));
    }

    #[test]
    fn invalid_config_is_an_error_and_zero_depth_is_rejected() {
        assert!(parse_config("search_dirs = 1").is_err());
        assert!(parse_config(r#"search_dirs = [{ path = "/repos", depth = 0 }]"#).is_err());
    }
}
