//! How kiosk was started: by the herdr TUI (as a plugin), by the Herdr desktop app's
//! `kiosk` mod (the `app` command), or by neither.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::context::PluginContext;

/// Set by the herdr TUI for plugin processes (and every pane it runs).
pub const HERDR_BIN_PATH: &str = "HERDR_BIN_PATH";
pub const HERDR_PLUGIN_CONTEXT_JSON: &str = "HERDR_PLUGIN_CONTEXT_JSON";

/// The command the Herdr app's `kiosk` mod runs.
pub const APP_COMMAND: &str = "app";

const USAGE: &str = "usage: herdr-kiosk [app [--app-home <folder>] [--context <json>]]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// The picker in this terminal.
    Terminal(Host),
    /// The picker for the Herdr app, which draws it natively.
    App(AppArgs),
}

/// Who started the terminal picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// Neither herdr nor the Herdr app started kiosk, so it has no backend.
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

/// What the app mod passes the `app` command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppArgs {
    /// The app's home (`HERDR_APP_HOME`), which selects the window `herdr-app` drives.
    pub app_home: Option<PathBuf>,
    /// Where the user is in the app.
    pub context: PluginContext,
}

pub fn invocation() -> Result<Invocation> {
    invocation_with(std::env::args_os().skip(1), |name| std::env::var_os(name))
}

pub fn invocation_with(
    args: impl IntoIterator<Item = OsString>,
    get_env: impl Fn(&str) -> Option<OsString>,
) -> Result<Invocation> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return detect_host_with(get_env).map(Invocation::Terminal);
    };
    if command != APP_COMMAND {
        bail!("unknown command {}; {USAGE}", command.to_string_lossy());
    }
    let mut app = AppArgs::default();
    while let Some(flag) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .with_context(|| format!("{flag} needs a value; {USAGE}"))
        };
        match flag.to_str() {
            Some("--app-home") => {
                let path = PathBuf::from(value("--app-home")?);
                if !path.is_absolute() {
                    bail!("--app-home must be an absolute path: {}", path.display());
                }
                app.app_home = Some(path);
            }
            Some("--context") => {
                let json = value("--context")?;
                app.context = context_from(Some(json), "--context")?;
            }
            _ => bail!("unknown option {}; {USAGE}", flag.to_string_lossy()),
        }
    }
    Ok(Invocation::App(app))
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

/// The program `name` on `PATH`, as a shell would find it.
pub fn find_program(name: &str) -> Result<PathBuf> {
    find_program_in(name, std::env::var_os("PATH"), std::env::var_os("PATHEXT"))
}

fn find_program_in(
    name: &str,
    path: Option<OsString>,
    pathext: Option<OsString>,
) -> Result<PathBuf> {
    let extensions: Vec<String> = if cfg!(windows) {
        pathext
            .and_then(|value| value.into_string().ok())
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path.unwrap_or_default())
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| {
            extensions
                .iter()
                .map(move |extension| dir.join(format!("{name}{extension}")))
        })
        .find(|candidate| is_executable(candidate))
        .with_context(|| format!("{name} is not on PATH"))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
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

    fn invoke(args: &[&str]) -> Result<Invocation> {
        invocation_with(args.iter().map(OsString::from), |_| None)
    }

    #[cfg(not(windows))]
    const APP_HOME: &str = "/home/me/.config/herdr-app";
    #[cfg(windows)]
    const APP_HOME: &str = r"C:\Users\me\.config\herdr-app";

    #[test]
    fn no_host_variables_means_standalone() {
        assert_eq!(detect(&[]).unwrap(), Host::Standalone);
        assert_eq!(detect(&[(HERDR_BIN_PATH, "")]).unwrap(), Host::Standalone);
        assert_eq!(invoke(&[]).unwrap(), Invocation::Terminal(Host::Standalone));
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

    #[test]
    fn the_app_command_reads_its_home_and_context() {
        let invocation = invoke(&[
            APP_COMMAND,
            "--app-home",
            APP_HOME,
            "--context",
            r#"{"focused_pane_cwd":"/repo/src"}"#,
        ])
        .unwrap();
        let Invocation::App(app) = invocation else {
            panic!("{invocation:?}");
        };
        assert_eq!(app.app_home.as_deref(), Some(Path::new(APP_HOME)));
        assert_eq!(app.context.current_cwd(), Some(Path::new("/repo/src")));
        assert_eq!(
            invoke(&[APP_COMMAND]).unwrap(),
            Invocation::App(AppArgs::default())
        );
    }

    #[test]
    fn the_app_command_ignores_the_herdr_tuis_variables() {
        let invocation = invocation_with([OsString::from(APP_COMMAND)], |name| {
            (name == HERDR_BIN_PATH).then(|| "/bin/herdr".into())
        })
        .unwrap();
        assert_eq!(invocation, Invocation::App(AppArgs::default()));
    }

    #[test]
    fn bad_app_arguments_are_refused() {
        for (args, expected) in [
            (&["serve"][..], "unknown command"),
            (&[APP_COMMAND, "--verbose"], "unknown option"),
            (&[APP_COMMAND, "--app-home"], "needs a value"),
            (&[APP_COMMAND, "--app-home", "relative"], "absolute"),
            (&[APP_COMMAND, "--context", "not json"], "--context"),
        ] {
            let error = invoke(args).unwrap_err().to_string();
            assert!(error.contains(expected), "{args:?}: {error}");
        }
    }

    #[test]
    fn programs_are_found_on_path() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let name = "kiosk-test-program";
        let file_name = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        };
        let program = second.path().join(&file_name);
        std::fs::write(&program, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let not_executable = first.path().join(&file_name);
            std::fs::write(&not_executable, "").unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([first.path(), second.path()]).unwrap();
        assert_eq!(
            find_program_in(name, Some(path.clone()), None).unwrap(),
            program
        );
        let error = find_program_in("not-a-kiosk-program", Some(path), None).unwrap_err();
        assert!(error.to_string().contains("not on PATH"), "{error}");
    }
}
