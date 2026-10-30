//! Which host kiosk was launched from, read from its environment: the herdr TUI
//! (as a plugin), the Herdr desktop app (through its `kiosk` mod), or neither.

use std::{ffi::OsString, path::PathBuf};

use anyhow::{Result, bail};

use crate::context::PluginContext;

/// Set by the herdr TUI for plugin processes (and every pane it runs).
pub const HERDR_BIN_PATH: &str = "HERDR_BIN_PATH";
pub const HERDR_PLUGIN_CONTEXT_JSON: &str = "HERDR_PLUGIN_CONTEXT_JSON";
const HERDR_PLUGIN_DIRS: [&str; 2] = ["HERDR_PLUGIN_CONFIG_DIR", "HERDR_PLUGIN_STATE_DIR"];

/// Set by the Herdr app's `kiosk` mod.
pub const KIOSK_APP_BIN: &str = "HERDR_KIOSK_APP_BIN";
pub const KIOSK_HC_BIN: &str = "HERDR_KIOSK_HC_BIN";
pub const KIOSK_NOTICE_FILE: &str = "HERDR_KIOSK_NOTICE_FILE";
pub const KIOSK_CONTEXT_JSON: &str = "HERDR_KIOSK_CONTEXT_JSON";
const KIOSK_APP_VARS: [&str; 4] = [
    KIOSK_APP_BIN,
    KIOSK_HC_BIN,
    KIOSK_NOTICE_FILE,
    KIOSK_CONTEXT_JSON,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// Neither herdr nor the Herdr app started kiosk, so it has no backend.
    Standalone,
    HerdrTui {
        herdr_binary: PathBuf,
        context: PluginContext,
    },
    HerdrApp {
        herdr_app_binary: PathBuf,
        hc_binary: PathBuf,
        /// Where notifications go for the mod to show once kiosk exits.
        notice_file: PathBuf,
        context: PluginContext,
    },
}

impl Host {
    pub fn context(&self) -> Option<&PluginContext> {
        match self {
            Self::Standalone => None,
            Self::HerdrTui { context, .. } | Self::HerdrApp { context, .. } => Some(context),
        }
    }
}

pub fn detect_host() -> Result<Host> {
    detect_host_with(|name| std::env::var_os(name))
}

pub fn detect_host_with(get_env: impl Fn(&str) -> Option<OsString>) -> Result<Host> {
    let get = |name: &str| get_env(name).filter(|value| !value.is_empty());
    let tui = get(HERDR_BIN_PATH);
    let app = get(KIOSK_APP_BIN);
    match (tui, app) {
        (Some(_), Some(_)) => bail!(
            "both {HERDR_BIN_PATH} (herdr TUI) and {KIOSK_APP_BIN} (Herdr app) are set; \
             the Herdr app's kiosk mod unsets the herdr TUI's variables, so start kiosk from it"
        ),
        (Some(herdr_binary), None) => {
            if let Some(name) = KIOSK_APP_VARS.into_iter().find(|name| get(name).is_some()) {
                bail!("{name} is set for the Herdr app, but kiosk was started by the herdr TUI");
            }
            Ok(Host::HerdrTui {
                herdr_binary: herdr_binary.into(),
                context: context_from(get(HERDR_PLUGIN_CONTEXT_JSON), HERDR_PLUGIN_CONTEXT_JSON)?,
            })
        }
        (None, Some(herdr_app_binary)) => {
            if let Some(name) = [HERDR_PLUGIN_CONTEXT_JSON]
                .into_iter()
                .chain(HERDR_PLUGIN_DIRS)
                .find(|name| get(name).is_some())
            {
                bail!("{name} is set for the herdr TUI, but kiosk was started by the Herdr app");
            }
            let absolute = |name: &str, value: Option<OsString>| -> Result<PathBuf> {
                let Some(value) = value else {
                    bail!("{name} must be set when {KIOSK_APP_BIN} is");
                };
                let path = PathBuf::from(value);
                if !path.is_absolute() {
                    bail!("{name} must be an absolute path: {}", path.display());
                }
                Ok(path)
            };
            Ok(Host::HerdrApp {
                herdr_app_binary: absolute(KIOSK_APP_BIN, Some(herdr_app_binary))?,
                hc_binary: absolute(KIOSK_HC_BIN, get(KIOSK_HC_BIN))?,
                notice_file: absolute(KIOSK_NOTICE_FILE, get(KIOSK_NOTICE_FILE))?,
                context: context_from(get(KIOSK_CONTEXT_JSON), KIOSK_CONTEXT_JSON)?,
            })
        }
        (None, None) => Ok(Host::Standalone),
    }
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
    use std::{collections::HashMap, path::Path};

    use super::*;

    fn detect(vars: &[(&str, &str)]) -> Result<Host> {
        let vars: HashMap<String, OsString> = vars
            .iter()
            .map(|(name, value)| ((*name).to_string(), OsString::from(value)))
            .collect();
        detect_host_with(|name| vars.get(name).cloned())
    }

    const APP: [(&str, &str); 4] = [
        (KIOSK_APP_BIN, "/bin/herdr-app"),
        (KIOSK_HC_BIN, "/bin/hc"),
        (KIOSK_NOTICE_FILE, "/tmp/notices.jsonl"),
        (KIOSK_CONTEXT_JSON, r#"{"focused_pane_cwd":"/repo/src"}"#),
    ];

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
    fn herdr_app_reads_its_binaries_notice_file_and_context() {
        let host = detect(&APP).unwrap();
        assert_eq!(
            host,
            Host::HerdrApp {
                herdr_app_binary: "/bin/herdr-app".into(),
                hc_binary: "/bin/hc".into(),
                notice_file: "/tmp/notices.jsonl".into(),
                context: PluginContext {
                    focused_pane_cwd: Some("/repo/src".into()),
                    workspace_cwd: None,
                },
            }
        );
        assert_eq!(
            host.context().unwrap().current_cwd(),
            Some(Path::new("/repo/src"))
        );
    }

    #[test]
    fn both_hosts_at_once_is_an_error() {
        let mut vars = APP.to_vec();
        vars.push((HERDR_BIN_PATH, "/bin/herdr"));
        let error = detect(&vars).unwrap_err().to_string();
        assert!(error.contains("both"), "{error}");
    }

    #[test]
    fn the_other_hosts_variables_are_rejected() {
        let error = detect(&[(HERDR_BIN_PATH, "/bin/herdr"), (KIOSK_NOTICE_FILE, "/n")])
            .unwrap_err()
            .to_string();
        assert!(error.contains(KIOSK_NOTICE_FILE), "{error}");

        for name in [
            HERDR_PLUGIN_CONTEXT_JSON,
            "HERDR_PLUGIN_CONFIG_DIR",
            "HERDR_PLUGIN_STATE_DIR",
        ] {
            let mut vars = APP.to_vec();
            vars.push((name, "{}"));
            let error = detect(&vars).unwrap_err().to_string();
            assert!(error.contains(name), "{error}");
        }
    }

    #[test]
    fn herdr_app_requires_every_absolute_path() {
        for missing in [KIOSK_HC_BIN, KIOSK_NOTICE_FILE] {
            let vars: Vec<_> = APP
                .into_iter()
                .filter(|(name, _)| *name != missing)
                .collect();
            let error = detect(&vars).unwrap_err().to_string();
            assert!(error.contains(missing), "{error}");
        }
        let mut vars = APP.to_vec();
        vars[1] = (KIOSK_HC_BIN, "hc");
        let error = detect(&vars).unwrap_err().to_string();
        assert!(error.contains("absolute"), "{error}");
    }

    #[test]
    fn invalid_context_is_reported_with_its_variable() {
        let mut vars = APP.to_vec();
        vars[3] = (KIOSK_CONTEXT_JSON, "not json");
        let error = detect(&vars).unwrap_err().to_string();
        assert!(error.contains(KIOSK_CONTEXT_JSON), "{error}");
    }
}
