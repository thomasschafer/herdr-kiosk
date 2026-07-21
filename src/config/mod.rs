use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const APP_NAME: &str = "herdr-kiosk";
pub const DEFAULT_SEARCH_DEPTH: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SearchDirEntry {
    Simple(String),
    Rich { path: String, depth: Option<u16> },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeysConfig {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeConfig {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub search_dirs: Vec<SearchDirEntry>,
    pub keys: KeysConfig,
    pub theme: ThemeConfig,
}

impl Config {
    pub fn resolved_search_dirs_with(
        &self,
        home: Option<&Path>,
        is_dir: impl Fn(&Path) -> bool,
    ) -> Result<Vec<(PathBuf, u16)>> {
        self.search_dirs
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
            .collect::<Result<Vec<_>>>()
            .map(|dirs| dirs.into_iter().filter(|(path, _)| is_dir(path)).collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWarning {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: Config,
    pub path: Option<PathBuf>,
    pub warnings: Vec<ConfigWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPathResolution {
    pub path: Option<PathBuf>,
    pub warnings: Vec<ConfigWarning>,
}

pub fn resolve_config_path(get_env: impl Fn(&str) -> Option<String>) -> ConfigPathResolution {
    let mut warnings = Vec::new();
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

    for candidate in candidates.into_iter().flatten() {
        let (source, base, add_app_dir) = candidate;
        if !base.is_absolute() {
            // A cwd-relative fallback lets a browsed repository supply plugin
            // config, so only absolute environment-derived locations are trusted.
            warnings.push(ConfigWarning {
                message: format!(
                    "refusing relative config directory from {source}: {}",
                    base.display()
                ),
            });
            continue;
        }
        let path = if add_app_dir {
            base.join(APP_NAME).join("config.toml")
        } else {
            base.join("config.toml")
        };
        return ConfigPathResolution {
            path: Some(path),
            warnings,
        };
    }

    ConfigPathResolution {
        path: None,
        warnings,
    }
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
            warnings: resolution.warnings,
        });
    };
    let Some(contents) =
        read_file(&path).with_context(|| format!("failed to read config {}", path.display()))?
    else {
        return Ok(LoadedConfig {
            config: Config::default(),
            path: Some(path),
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
    Ok(())
}

fn expand_tilde(path: &str, home: Option<&Path>) -> Result<PathBuf> {
    if path == "~" {
        return home
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("cannot expand '~' because HOME is unavailable"));
    }
    if let Some(rest) = path.strip_prefix("~/") {
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
    fn expands_tilde_with_injected_home_and_preserves_depth() {
        let (config, _) =
            parse_config(r#"search_dirs = ["~", { path = "~/Work", depth = 4 }, "/absolute"]"#)
                .unwrap();
        let resolved = config
            .resolved_search_dirs_with(Some(Path::new("/home/tester")), |_| true)
            .unwrap();
        assert_eq!(
            resolved,
            [
                (PathBuf::from("/home/tester"), 1),
                (PathBuf::from("/home/tester/Work"), 4),
                (PathBuf::from("/absolute"), 1),
            ]
        );
    }

    #[test]
    fn missing_home_makes_tilde_expansion_fail_loudly() {
        let (config, _) = parse_config(r#"search_dirs = ["~/Work"]"#).unwrap();
        assert!(config.resolved_search_dirs_with(None, |_| true).is_err());
    }

    #[test]
    fn config_path_uses_documented_precedence() {
        let values = HashMap::from([
            ("HERDR_PLUGIN_CONFIG_DIR", "/plugin/config"),
            ("XDG_CONFIG_HOME", "/xdg"),
            ("HOME", "/home/tester"),
        ]);
        let resolution = resolve_config_path(|name| values.get(name).map(ToString::to_string));
        assert_eq!(
            resolution.path,
            Some(PathBuf::from("/plugin/config/config.toml"))
        );
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
        let values = HashMap::from([
            ("HERDR_PLUGIN_CONFIG_DIR", "plugin-config"),
            ("XDG_CONFIG_HOME", "/xdg"),
        ]);
        let resolution = resolve_config_path(|name| values.get(name).map(ToString::to_string));
        assert_eq!(
            resolution.path,
            Some(PathBuf::from("/xdg/herdr-kiosk/config.toml"))
        );
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
    fn missing_config_loads_empty_defaults_without_reading_relative_paths() {
        let loaded = load_config_with(|_| None, |_| panic!("no path should be read")).unwrap();
        assert!(loaded.config.search_dirs.is_empty());
        assert!(loaded.path.is_none());
    }

    #[test]
    fn injected_reader_loads_config() {
        let loaded = load_config_with(
            |name| (name == "HOME").then(|| "/home/tester".into()),
            |path| {
                assert_eq!(
                    path,
                    Path::new("/home/tester/.config/herdr-kiosk/config.toml")
                );
                Ok(Some("search_dirs = [\"/repos\"]".into()))
            },
        )
        .unwrap();
        assert_eq!(loaded.config.search_dirs.len(), 1);
    }

    #[test]
    fn invalid_config_is_an_error_and_zero_depth_is_rejected() {
        assert!(parse_config("search_dirs = 1").is_err());
        assert!(parse_config(r#"search_dirs = [{ path = "/repos", depth = 0 }]"#).is_err());
    }
}
