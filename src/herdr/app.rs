//! The Herdr desktop app as a backend. The app has no worktree API, so kiosk does the
//! Git work and this backend drives the window through the `herdr-app` CLI (spaces,
//! splits, focus) and herdr-core's `hc` (terminal folders). Kiosk sessions are app
//! spaces: a space counts as showing a checkout when its first terminal's live folder is
//! inside it, the rule the app's own `worktree` mod uses.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use kiosk_core::{
    backend::{
        Backend, CheckoutState, ManagedWorktrees, Opened, WorktreePlacement, Worktrees,
        identity::{LiveFolder, session_showing_checkout},
    },
    git::GitProvider,
};

use super::{
    HerdrError, PaneOpenError, PaneSplitRequest, PaneSplitResponse, focus_or_create_folder,
    on_open, path_arg, require_absolute, require_nonempty,
};
use kiosk_core::config::OnOpenConfig;

/// Where new checkouts go, below the user's home: the same place as the herdr
/// TUI's default `worktrees.directory` and the app's `worktree` mod.
pub const WORKTREE_ROOT: &str = ".herdr/worktrees";

/// The two programs the app backend runs.
pub trait AppHost: Send + Sync {
    /// Runs `herdr-app` and returns its standard output.
    fn herdr_app(&self, args: &[String]) -> Result<Vec<u8>, HerdrError>;
    /// Runs herdr-core's `hc` and returns its standard output.
    fn hc(&self, args: &[String]) -> Result<Vec<u8>, HerdrError>;
}

#[derive(Debug, Clone)]
pub struct CliAppHost {
    herdr_app: PathBuf,
    hc: PathBuf,
}

impl CliAppHost {
    pub fn new(herdr_app: impl Into<PathBuf>, hc: impl Into<PathBuf>) -> Self {
        Self {
            herdr_app: herdr_app.into(),
            hc: hc.into(),
        }
    }
}

fn run_program(binary: &Path, args: &[String]) -> Result<std::process::Output, HerdrError> {
    Command::new(binary).args(args).output().map_err(|error| {
        HerdrError::Invocation(format!("could not execute {}: {error}", binary.display()))
    })
}

fn exit_failure(binary: &Path, output: &std::process::Output) -> HerdrError {
    HerdrError::Invocation(format!(
        "{} exited with {}: {}",
        binary.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

impl AppHost for CliAppHost {
    fn herdr_app(&self, args: &[String]) -> Result<Vec<u8>, HerdrError> {
        let output = run_program(&self.herdr_app, args)?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        // `herdr-app do` reports refusals as `{ "error": "..." }` on stdout.
        match serde_json::from_slice::<AppError>(&output.stdout) {
            Ok(AppError { error }) => Err(HerdrError::Other {
                code: "herdr_app".into(),
                message: error,
            }),
            Err(_) => Err(exit_failure(&self.herdr_app, &output)),
        }
    }

    fn hc(&self, args: &[String]) -> Result<Vec<u8>, HerdrError> {
        let output = run_program(&self.hc, args)?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        match serde_json::from_slice::<HcError>(&output.stderr) {
            Ok(HcError { error }) => Err(HerdrError::Other {
                code: error.kind,
                message: error.message,
            }),
            Err(_) => Err(exit_failure(&self.hc, &output)),
        }
    }
}

#[derive(Deserialize)]
struct AppError {
    error: String,
}

#[derive(Deserialize)]
struct HcError {
    error: HcErrorBody,
}

#[derive(Deserialize)]
struct HcErrorBody {
    kind: String,
    message: String,
}

#[derive(Deserialize)]
struct AppList {
    spaces: Vec<AppSpace>,
}

#[derive(Deserialize)]
struct AppSpace {
    id: String,
    #[serde(default)]
    machine: Option<String>,
    tabs: Vec<AppTab>,
}

#[derive(Deserialize)]
struct AppTab {
    terminals: Vec<AppTerminal>,
}

#[derive(Deserialize)]
struct AppTerminal {
    terminal: String,
    #[serde(default)]
    pane: Option<String>,
}

#[derive(Deserialize)]
struct SpaceCreated {
    space: String,
    terminal: String,
}

#[derive(Deserialize)]
struct PaneCreated {
    pane: String,
}

#[derive(Deserialize)]
struct HcTerminal {
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    foreground: Option<HcForeground>,
}

#[derive(Deserialize)]
struct HcForeground {
    #[serde(default)]
    cwd: Option<PathBuf>,
}

#[derive(Serialize)]
struct Notice<'a> {
    title: &'a str,
    body: &'a str,
}

/// A space on this machine, with the live folder of each of its panes' terminals in the
/// app's order.
#[derive(Debug, Clone)]
struct Space {
    id: String,
    folders: Vec<Option<PathBuf>>,
}

impl Space {
    /// Where the space is: the folder of its first pane with a known one.
    fn place(&self) -> Option<PathBuf> {
        self.folders
            .iter()
            .flatten()
            .next()
            .map(|folder| kiosk_core::path::canonical_or_original(folder))
    }
}

/// An app space id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpaceId(pub String);

/// Each space with a known folder and where it is.
fn space_places(spaces: &[Space]) -> Vec<LiveFolder<SpaceId>> {
    spaces
        .iter()
        .filter_map(|space| {
            Some(LiveFolder {
                session: SpaceId(space.id.clone()),
                folder: space.place()?,
            })
        })
        .collect()
}

pub struct HerdrApp {
    host: Arc<dyn AppHost>,
    git: Arc<dyn GitProvider>,
    placement: WorktreePlacement,
    notice_file: PathBuf,
    on_open: OnOpenConfig,
}

impl HerdrApp {
    /// New checkouts go in `placement`, and notifications are appended to `notice_file` as
    /// JSON lines for the app mod to show.
    pub fn new(
        host: Arc<dyn AppHost>,
        git: Arc<dyn GitProvider>,
        placement: WorktreePlacement,
        notice_file: PathBuf,
        on_open: OnOpenConfig,
    ) -> Self {
        Self {
            host,
            git,
            placement,
            notice_file,
            on_open,
        }
    }

    fn command(&self, name: &str, args: &Value) -> Result<Vec<u8>, HerdrError> {
        self.host
            .herdr_app(&["do".into(), name.into(), args.to_string()])
    }

    fn command_json<T: DeserializeOwned>(&self, name: &str, args: &Value) -> Result<T, HerdrError> {
        let output = self.command(name, args)?;
        parse(&output, &format!("herdr-app do {name}"))
    }

    fn list(&self) -> Result<AppList, HerdrError> {
        let output = self.host.herdr_app(&["ls".into()])?;
        parse(&output, "herdr-app ls")
    }

    fn terminal(&self, address: &str) -> Result<HcTerminal, HerdrError> {
        let output = self
            .host
            .hc(&["get".into(), address.into(), "--json".into()])?;
        parse(&output, "hc get")
    }

    /// The spaces on this machine. Spaces on other machines are left out: their
    /// folders are not paths here.
    fn spaces(&self) -> Result<Vec<Space>, HerdrError> {
        let spaces: Vec<(String, Vec<String>)> = self
            .list()?
            .spaces
            .into_iter()
            .filter(|space| space.machine.is_none())
            .map(|space| {
                let terminals = space
                    .tabs
                    .into_iter()
                    .flat_map(|tab| tab.terminals)
                    .filter(|terminal| terminal.pane.is_some())
                    .map(|terminal| terminal.terminal)
                    .collect();
                (space.id, terminals)
            })
            .collect();
        let terminals: Vec<&str> = spaces
            .iter()
            .flat_map(|(_, terminals)| terminals.iter().map(String::as_str))
            .collect();
        let mut folders = self.live_folders(&terminals)?.into_iter();
        Ok(spaces
            .into_iter()
            .map(|(id, terminals)| Space {
                id,
                folders: terminals
                    .iter()
                    .map(|_| folders.next().expect("a folder for every terminal"))
                    .collect(),
            })
            .collect())
    }

    /// Each terminal's live folder, in order. One `hc get` each, all at once:
    /// one after another they add up to most of the time an open takes.
    fn live_folders(&self, terminals: &[&str]) -> Result<Vec<Option<PathBuf>>, HerdrError> {
        std::thread::scope(|scope| {
            let lookups: Vec<_> = terminals
                .iter()
                .map(|terminal| {
                    scope.spawn(move || {
                        let info = self.terminal(terminal)?;
                        Ok(info.foreground.and_then(|f| f.cwd).or(info.cwd))
                    })
                })
                .collect();
            lookups
                .into_iter()
                .map(|lookup| {
                    lookup
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                })
                .collect()
        })
    }

    /// Makes a space whose first terminal starts in `folder`, shown at once. It is
    /// left unnamed so the app names it after its live folder, as its own
    /// `worktree` mod does (which also nests worktree spaces under their repo).
    /// Returns the space and its first pane.
    fn open_space(&self, folder: &Path) -> Result<(String, String), HerdrError> {
        let folder = path_arg("folder", folder)?;
        let created: SpaceCreated = self.command_json("space.new", &json!({ "folder": folder }))?;
        let pane = self
            .list()?
            .spaces
            .into_iter()
            .filter(|space| space.id == created.space)
            .flat_map(|space| space.tabs)
            .flat_map(|tab| tab.terminals)
            .find(|terminal| terminal.terminal == created.terminal)
            .and_then(|terminal| terminal.pane)
            .ok_or_else(|| {
                HerdrError::InvalidResponse(format!(
                    "the new space {} has no pane for its terminal {}",
                    created.space, created.terminal
                ))
            })?;
        Ok((created.space, pane))
    }

    fn select_space(&self, id: &str) -> Result<(), HerdrError> {
        require_nonempty("space id", id)?;
        self.command("space.select", &json!({ "id": id }))?;
        Ok(())
    }

    /// One `pane.split` that starts the new terminal in the checkout with the
    /// command typed ahead into its shell: the shell runs it as soon as it is
    /// ready, keeps it in its history, and stays when it ends.
    fn pane_open(
        &self,
        request: &PaneSplitRequest,
        command: &str,
    ) -> Result<PaneSplitResponse, PaneOpenError> {
        self.split(request, command).map_err(PaneOpenError::Split)
    }

    fn split(
        &self,
        request: &PaneSplitRequest,
        command: &str,
    ) -> Result<PaneSplitResponse, HerdrError> {
        require_nonempty("pane id", &request.pane_id)?;
        require_nonempty("command", command)?;
        require_absolute("cwd", &request.cwd)?;
        let folder = path_arg("cwd", &request.cwd)?;
        if request.ratio.is_some() {
            return Err(HerdrError::InvalidArgument(
                "the Herdr app always splits panes in half; remove `ratio` from this [on_open] pane"
                    .into(),
            ));
        }
        let created: PaneCreated = self.command_json(
            "pane.split",
            &json!({
                "direction": request.direction.as_str(),
                "pane": request.pane_id,
                "folder": folder,
                "type": command,
            }),
        )?;
        if !request.focus {
            self.command("pane.focus", &json!({ "pane": request.pane_id }))?;
        }
        Ok(PaneSplitResponse {
            pane_id: created.pane,
        })
    }

    fn checkout_paths(&self, repo: &Path) -> Result<Vec<PathBuf>, HerdrError> {
        self.git
            .list_worktrees(repo)
            .map(|listed| {
                listed
                    .items
                    .into_iter()
                    .map(|worktree| worktree.path)
                    .collect()
            })
            .map_err(|error| git_error(&error))
    }
}

impl Backend for HerdrApp {
    type SessionId = SpaceId;
    type Error = HerdrError;

    fn open_repos(&self) -> Result<Vec<PathBuf>, HerdrError> {
        self.spaces()?
            .iter()
            .filter_map(Space::place)
            .filter_map(|place| {
                self.git
                    .main_checkout(&place)
                    .map_err(|error| git_error(&error))
                    .transpose()
            })
            .collect()
    }

    fn open_folders(&self) -> Result<Vec<PathBuf>, HerdrError> {
        Ok(self
            .spaces()?
            .into_iter()
            .flat_map(|space| space.folders)
            .flatten()
            .collect())
    }

    fn checkouts(&self, repo: &Path) -> Result<Vec<CheckoutState<SpaceId>>, HerdrError> {
        require_absolute("repository", repo)?;
        let checkouts = self.checkout_paths(repo)?;
        let places = space_places(&self.spaces()?);
        Ok(checkouts
            .iter()
            .map(|path| CheckoutState {
                path: path.clone(),
                session: session_showing_checkout(&places, path, &checkouts).cloned(),
            })
            .collect())
    }

    fn open_checkout(&self, repo: &Path, checkout: &Path) -> Result<Opened, HerdrError> {
        require_absolute("checkout", checkout)?;
        let checkouts = self.checkout_paths(repo)?;
        let places = space_places(&self.spaces()?);
        if let Some(space) = session_showing_checkout(&places, checkout, &checkouts) {
            self.select_space(&space.0)?;
            return Ok(Opened::default());
        }
        // Git's spelling of the path, which the app names the space after.
        let canonical = kiosk_core::path::canonical_or_original(checkout);
        let checkout = checkouts
            .iter()
            .find(|path| {
                kiosk_core::path::equivalent(
                    &kiosk_core::path::canonical_or_original(path),
                    &canonical,
                )
            })
            .map_or(checkout, PathBuf::as_path);
        let (_, pane) = self.open_space(checkout)?;
        Ok(Opened {
            warning: on_open::apply(&self.on_open, &pane, checkout, |request, command| {
                self.pane_open(request, command)
            }),
        })
    }

    fn open_folder(&self, folder: &Path) -> Result<Opened, HerdrError> {
        require_absolute("folder", folder)?;
        let panes: Vec<_> = self
            .spaces()?
            .into_iter()
            .flat_map(|space| {
                space
                    .folders
                    .into_iter()
                    .flatten()
                    .map(move |folder| LiveFolder {
                        session: space.id.clone(),
                        folder,
                    })
            })
            .collect();
        let warning = focus_or_create_folder(
            &panes,
            folder,
            |space| self.select_space(space),
            || self.open_space(folder).map(|_| None),
        )?;
        Ok(Opened { warning })
    }

    fn notify(&self, title: &str, body: &str) -> Result<(), HerdrError> {
        require_nonempty("notification title", title)?;
        require_nonempty("notification body", body)?;
        let notice_error = |error: std::io::Error| {
            HerdrError::Invocation(format!(
                "could not write notification to {}: {error}",
                self.notice_file.display()
            ))
        };
        if let Some(parent) = self.notice_file.parent() {
            fs::create_dir_all(parent).map_err(notice_error)?;
        }
        let mut line =
            serde_json::to_string(&Notice { title, body }).expect("a notice always serializes");
        line.push('\n');
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.notice_file)
            .and_then(|mut file| file.write_all(line.as_bytes()))
            .map_err(notice_error)
    }

    fn worktrees(&self) -> Worktrees<'_, Self> {
        Worktrees::Managed(self)
    }
}

impl ManagedWorktrees<Self> for HerdrApp {
    fn placement(&self) -> &WorktreePlacement {
        &self.placement
    }

    fn close(&self, space: &SpaceId) -> Result<(), HerdrError> {
        require_nonempty("space id", &space.0)?;
        self.command("space.close", &json!({ "id": space.0, "confirmed": true }))?;
        Ok(())
    }
}

fn parse<T: DeserializeOwned>(output: &[u8], what: &str) -> Result<T, HerdrError> {
    serde_json::from_slice(output).map_err(|error| {
        HerdrError::InvalidResponse(format!(
            "{what}: {error}; stdout was {:?}",
            String::from_utf8_lossy(output).trim()
        ))
    })
}

fn git_error(error: &anyhow::Error) -> HerdrError {
    HerdrError::Other {
        code: "git".into(),
        message: format!("{error:#}"),
    }
}

#[cfg(test)]
mod tests;
