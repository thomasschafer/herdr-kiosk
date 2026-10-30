//! The Herdr desktop app as a backend. The app has no worktree API, so this
//! provider does the Git work itself and drives the window through the
//! `herdr-app` CLI (spaces, splits, focus) and herdr-core's `hc` (terminal
//! folders). Kiosk workspaces map to app spaces: a space counts as showing a
//! checkout when its first terminal's live folder is inside it, the rule the
//! app's own `worktree` mod uses.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use super::{
    HerdrError, HerdrProvider, OpenedWorktree, PaneInfo, PaneOpenError, PaneSplitRequest,
    PaneSplitResponse, WorkspaceCreateResponse, WorkspaceInfo, WorkspaceWorktreeInfo,
    WorktreeCreateRequest, WorktreeCreateResponse, WorktreeInfo, WorktreeListResponse,
    WorktreeOpenResponse, WorktreeOpenTarget, WorktreeRemoveResponse, path_arg, require_absolute,
    require_nonempty,
};
use crate::git::{GitProvider, Worktree, is_dirty_worktree_requires_force};

/// Where new checkouts go, below the user's home: the same place as the herdr
/// TUI's default `worktrees.directory` and the app's `worktree` mod.
pub const WORKTREE_ROOT: &str = ".herdr/worktrees";
const DEFAULT_WORKTREE_SLUG: &str = "worktree";

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

/// A space on this machine, with its panes in the app's order.
#[derive(Debug, Clone)]
struct Space {
    id: String,
    panes: Vec<Pane>,
}

#[derive(Debug, Clone)]
struct Pane {
    id: String,
    /// The terminal's live folder.
    cwd: Option<PathBuf>,
}

impl Space {
    /// The pane that decides where the space is: its first one with a known folder.
    fn first_pane(&self) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.cwd.is_some())
    }

    fn place(&self) -> Option<PathBuf> {
        self.first_pane()
            .and_then(|pane| pane.cwd.as_deref())
            .map(crate::path::canonical_or_original)
    }
}

pub struct AppHerdrProvider {
    host: Arc<dyn AppHost>,
    git: Arc<dyn GitProvider>,
    worktree_root: PathBuf,
    notice_file: PathBuf,
}

impl AppHerdrProvider {
    /// `worktree_root` holds new checkouts (`<root>/<repo>/<branch slug>`), and
    /// notifications are appended to `notice_file` as JSON lines for the app mod to show.
    pub fn new(
        host: Arc<dyn AppHost>,
        git: Arc<dyn GitProvider>,
        worktree_root: PathBuf,
        notice_file: PathBuf,
    ) -> Self {
        Self {
            host,
            git,
            worktree_root,
            notice_file,
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
        let spaces: Vec<(String, Vec<(String, String)>)> = self
            .list()?
            .spaces
            .into_iter()
            .filter(|space| space.machine.is_none())
            .map(|space| {
                let panes = space
                    .tabs
                    .into_iter()
                    .flat_map(|tab| tab.terminals)
                    .filter_map(|terminal| Some((terminal.pane?, terminal.terminal)))
                    .collect();
                (space.id, panes)
            })
            .collect();
        let terminals: Vec<&str> = spaces
            .iter()
            .flat_map(|(_, panes)| panes.iter().map(|(_, terminal)| terminal.as_str()))
            .collect();
        let mut folders = self.live_folders(&terminals)?.into_iter();
        Ok(spaces
            .into_iter()
            .map(|(id, panes)| Space {
                id,
                panes: panes
                    .into_iter()
                    .map(|(id, _)| Pane {
                        id,
                        cwd: folders.next().expect("a folder for every terminal"),
                    })
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

    fn worktrees(&self, cwd: &Path) -> Result<Vec<Worktree>, HerdrError> {
        self.git
            .list_worktrees(cwd)
            .map(|listed| listed.items)
            .map_err(|error| git_error(&error))
    }
}

/// The space showing `checkout`: its first terminal is inside it, and not inside a
/// deeper checkout of the same repository.
fn space_showing<'a>(spaces: &'a [Space], checkout: &Path, all: &[Worktree]) -> Option<&'a Space> {
    let checkout = crate::path::canonical_or_original(checkout);
    let deeper: Vec<PathBuf> = all
        .iter()
        .map(|worktree| crate::path::canonical_or_original(&worktree.path))
        .filter(|path| path != &checkout && crate::path::starts_with(path, &checkout))
        .collect();
    spaces.iter().find(|space| {
        space.place().is_some_and(|place| {
            crate::path::starts_with(&place, &checkout)
                && !deeper
                    .iter()
                    .any(|path| crate::path::starts_with(&place, path))
        })
    })
}

fn opened(root_pane_id: String, path: &Path) -> Result<OpenedWorktree, HerdrError> {
    Ok(OpenedWorktree {
        root_pane_id,
        path: path_arg("worktree path", path)?,
    })
}

/// The checkout of `worktrees` that contains `place`, the deepest one if they nest.
fn checkout_containing<'a>(worktrees: &'a [Worktree], place: &Path) -> Option<&'a Worktree> {
    worktrees
        .iter()
        .filter(|worktree| {
            crate::path::starts_with(place, &crate::path::canonical_or_original(&worktree.path))
        })
        .max_by_key(|worktree| worktree.path.components().count())
}

/// The folder name for a branch's checkout, as the herdr TUI makes it.
pub fn branch_slug(branch: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for character in branch.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        DEFAULT_WORKTREE_SLUG.into()
    } else {
        slug.into()
    }
}

impl HerdrProvider for AppHerdrProvider {
    fn worktree_open(
        &self,
        cwd: &Path,
        target: &WorktreeOpenTarget,
        focus: bool,
    ) -> Result<WorktreeOpenResponse, HerdrError> {
        require_absolute("cwd", cwd)?;
        require_focus(focus)?;
        let worktrees = self.worktrees(cwd)?;
        let checkout = match target {
            WorktreeOpenTarget::Path(path) => {
                require_absolute("worktree path", path)?;
                let path = crate::path::canonical_or_original(path);
                worktrees.iter().find(|worktree| {
                    crate::path::equivalent(
                        &crate::path::canonical_or_original(&worktree.path),
                        &path,
                    )
                })
            }
            WorktreeOpenTarget::Branch(branch) => {
                require_nonempty("branch", branch)?;
                worktrees
                    .iter()
                    .find(|worktree| worktree.branch.as_deref() == Some(branch.as_str()))
            }
        }
        .ok_or_else(|| {
            HerdrError::WorktreeNotFound(match target {
                WorktreeOpenTarget::Path(path) => format!(
                    "{} is not a checkout of this repository",
                    crate::path::display(path)
                ),
                WorktreeOpenTarget::Branch(branch) => format!("no checkout has {branch}"),
            })
        })?;

        let spaces = self.spaces()?;
        if let Some(space) = space_showing(&spaces, &checkout.path, &worktrees) {
            self.select_space(&space.id)?;
            let root = space
                .first_pane()
                .expect("a space showing a checkout has a pane with a folder");
            return Ok(WorktreeOpenResponse {
                opened: Some(opened(root.id.clone(), &checkout.path)?),
                already_open: Some(true),
                warning: None,
            });
        }
        let (_, pane) = self.open_space(&checkout.path)?;
        Ok(WorktreeOpenResponse {
            opened: Some(opened(pane, &checkout.path)?),
            already_open: Some(false),
            warning: None,
        })
    }

    fn worktree_create(
        &self,
        request: &WorktreeCreateRequest,
    ) -> Result<WorktreeCreateResponse, HerdrError> {
        require_absolute("cwd", &request.cwd)?;
        require_nonempty("branch", &request.branch)?;
        require_focus(request.focus)?;
        let repo = self
            .git
            .main_checkout(&request.cwd)
            .map_err(|error| git_error(&error))?
            .ok_or_else(|| {
                HerdrError::NotGitWorktree(format!(
                    "{} is not in a Git repository",
                    crate::path::display(&request.cwd)
                ))
            })?;
        let path = if let Some(path) = &request.path {
            require_absolute("worktree path", path)?;
            path.clone()
        } else {
            let repo_name = repo.file_name().ok_or_else(|| {
                HerdrError::InvalidArgument(format!(
                    "repository {} has no folder name",
                    crate::path::display(&repo)
                ))
            })?;
            self.worktree_root
                .join(repo_name)
                .join(branch_slug(&request.branch))
        };
        if let Some(base) = &request.base {
            require_nonempty("base", base)?;
        }
        self.git
            .add_worktree(&repo, &path, &request.branch, request.base.as_deref())
            .map_err(|error| HerdrError::WorktreeCreateFailed(format!("{error:#}")))?;
        let (_, pane) = self.open_space(&path)?;
        Ok(WorktreeCreateResponse {
            opened: Some(opened(pane, &path)?),
            warning: None,
        })
    }

    fn worktree_remove(
        &self,
        workspace_id: &str,
        force: bool,
    ) -> Result<WorktreeRemoveResponse, HerdrError> {
        require_nonempty("space id", workspace_id)?;
        let spaces = self.spaces()?;
        let space = spaces
            .iter()
            .find(|space| space.id == workspace_id)
            .ok_or_else(|| HerdrError::Other {
                code: "space_not_found".into(),
                message: format!("no space {workspace_id} on this machine"),
            })?;
        let place = space.place().ok_or_else(|| {
            HerdrError::NotGitWorktree(format!("space {workspace_id} has no known folder"))
        })?;
        let worktrees = self.worktrees(&place)?;
        let (main, linked) = worktrees.split_first().ok_or_else(|| {
            HerdrError::InvalidResponse("git listed no worktrees for the space".into())
        })?;
        let checkout = checkout_containing(linked, &place).ok_or_else(|| {
            HerdrError::NotGitWorktree(format!(
                "{} is the repository's main checkout, not a linked worktree",
                crate::path::display(&main.path)
            ))
        })?;
        // The checkout goes first: a refusal (uncommitted changes) leaves the space as it was.
        self.git
            .remove_worktree(&main.path, &checkout.path, force)
            .map_err(|error| {
                if is_dirty_worktree_requires_force(&error) {
                    HerdrError::DirtyWorktreeRequiresForce(error.to_string())
                } else {
                    HerdrError::Other {
                        code: "worktree_remove_failed".into(),
                        message: format!("{error:#}"),
                    }
                }
            })?;
        let warning = self
            .command(
                "space.close",
                &json!({ "id": workspace_id, "confirmed": true }),
            )
            .err()
            .map(|error| format!("checkout was removed, but its space did not close: {error}"));
        Ok(WorktreeRemoveResponse { warning })
    }

    fn worktree_list(&self, cwd: &Path) -> Result<WorktreeListResponse, HerdrError> {
        require_absolute("cwd", cwd)?;
        let worktrees = self.worktrees(cwd)?;
        let spaces = self.spaces()?;
        let worktrees = worktrees
            .iter()
            .map(|worktree| {
                Ok(WorktreeInfo {
                    path: path_arg("worktree path", &worktree.path)?,
                    branch: worktree.branch.clone(),
                    open_workspace_id: space_showing(&spaces, &worktree.path, &worktrees)
                        .map(|space| space.id.clone()),
                })
            })
            .collect::<Result<_, HerdrError>>()?;
        Ok(WorktreeListResponse { worktrees })
    }

    fn workspace_list(&self) -> Result<Vec<WorkspaceInfo>, HerdrError> {
        self.spaces()?
            .iter()
            .map(|space| {
                let repo = match space.place() {
                    Some(place) => self
                        .git
                        .main_checkout(&place)
                        .map_err(|error| git_error(&error))?,
                    None => None,
                };
                let worktree = repo
                    .map(|repo| {
                        Ok::<_, HerdrError>(WorkspaceWorktreeInfo {
                            repo_root: path_arg("repository root", &repo)?,
                        })
                    })
                    .transpose()?;
                Ok(WorkspaceInfo { worktree })
            })
            .collect()
    }

    fn pane_list(&self) -> Result<Vec<PaneInfo>, HerdrError> {
        self.spaces()?
            .into_iter()
            .flat_map(|space| {
                space.panes.into_iter().map(move |pane| {
                    Ok(PaneInfo {
                        workspace_id: space.id.clone(),
                        cwd: pane
                            .cwd
                            .as_deref()
                            .map(|cwd| path_arg("terminal folder", cwd))
                            .transpose()?,
                        foreground_cwd: None,
                    })
                })
            })
            .collect()
    }

    fn workspace_create(
        &self,
        cwd: &Path,
        focus: bool,
    ) -> Result<WorkspaceCreateResponse, HerdrError> {
        require_absolute("cwd", cwd)?;
        require_focus(focus)?;
        let (space, _) = self.open_space(cwd)?;
        Ok(WorkspaceCreateResponse {
            workspace_id: Some(space),
            warning: None,
        })
    }

    fn workspace_focus(&self, workspace_id: &str) -> Result<(), HerdrError> {
        self.select_space(workspace_id)
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

    fn notification_show(&self, title: &str, body: &str) -> Result<(), HerdrError> {
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

/// Kiosk always shows what it opens, and the app has no way not to.
fn require_focus(focus: bool) -> Result<(), HerdrError> {
    if focus {
        Ok(())
    } else {
        Err(HerdrError::InvalidArgument(
            "the Herdr app always shows a space it opens".into(),
        ))
    }
}

#[cfg(test)]
mod tests;
