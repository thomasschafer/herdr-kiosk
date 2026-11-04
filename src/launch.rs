//! Which host kiosk was launched from, read from its environment: the herdr TUI (as a
//! plugin), or none.

use std::{ffi::OsString, path::PathBuf};

use anyhow::{Result, bail};

use crate::context::PluginContext;

/// Set by the herdr TUI for plugin processes (and every pane it runs).
pub const HERDR_BIN_PATH: &str = "HERDR_BIN_PATH";
pub const HERDR_PLUGIN_CONTEXT_JSON: &str = "HERDR_PLUGIN_CONTEXT_JSON";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// herdr didn't start kiosk, so it has no backend.
    Standalone,
    HerdrTui {
        herdr_binary: PathBuf,
        context: PluginContext,
    },
}

impl Host {
    pub fn context(&self) -> Option<&PluginContext> {
        match self {
            Self::Standalone => None,
            Self::HerdrTui { context, .. } => Some(context),
        }
    }
}

pub fn detect_host() -> Result<Host> {
    detect_host_with(|name| std::env::var_os(name))
}

pub fn detect_host_with(get_env: impl Fn(&str) -> Option<OsString>) -> Result<Host> {
    let get = |name: &str| get_env(name).filter(|value| !value.is_empty());
    let Some(herdr_binary) = get(HERDR_BIN_PATH) else {
        return Ok(Host::Standalone);
    };
    Ok(Host::HerdrTui {
        herdr_binary: herdr_binary.into(),
        context: context_from(get(HERDR_PLUGIN_CONTEXT_JSON), HERDR_PLUGIN_CONTEXT_JSON)?,
    })
}

fn context_from(json: Option<OsString>, name: &str) -> Result<PluginContext> {
    let Some(json) = json else {
        return Ok(PluginContext::default());
    };
    let Some(json) = json.to_str() else {
        bail!("{name} is not valid UTF-8");
    };
    PluginContext::from_json(json).map_err(|error| anyhow::anyhow!("invalid {name}: {error}"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn detect(vars: &[(&str, &str)]) -> Result<Host> {
        let vars: HashMap<String, OsString> = vars
            .iter()
            .map(|(name, value)| ((*name).to_string(), OsString::from(value)))
            .collect();
        detect_host_with(|name| vars.get(name).cloned())
    }

    #[test]
    fn no_host_variables_means_standalone() {
        assert_eq!(detect(&[]).unwrap(), Host::Standalone);
        assert_eq!(detect(&[(HERDR_BIN_PATH, "")]).unwrap(), Host::Standalone);
    }

    #[test]
    fn herdr_tui_reads_its_binary_and_plugin_context() {
        let host = detect(&[
            (HERDR_BIN_PATH, "/bin/herdr"),
            (HERDR_PLUGIN_CONTEXT_JSON, r#"{"workspace_cwd":"/repo"}"#),
        ])
        .unwrap();
        assert_eq!(
            host,
            Host::HerdrTui {
                herdr_binary: "/bin/herdr".into(),
                context: PluginContext {
                    focused_pane_cwd: None,
                    workspace_cwd: Some("/repo".into()),
                },
            }
        );
    }

    #[test]
    fn invalid_plugin_context_is_reported_with_its_variable() {
        let error = detect(&[
            (HERDR_BIN_PATH, "/bin/herdr"),
            (HERDR_PLUGIN_CONTEXT_JSON, "not json"),
        ])
        .unwrap_err()
        .to_string();
        assert!(error.contains(HERDR_PLUGIN_CONTEXT_JSON), "{error}");
    }
}
