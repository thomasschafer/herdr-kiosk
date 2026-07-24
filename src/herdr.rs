use std::{
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, de::DeserializeOwned};

use crate::config::OnOpenPaneDirection;

pub mod mock;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceWorktreeInfo {
    pub repo_root: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceInfo {
    #[serde(default)]
    pub worktree: Option<WorkspaceWorktreeInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorktreeInfo {
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub open_workspace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeListResponse {
    pub worktrees: Vec<WorktreeInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCreateResponse {
    pub opened: Option<OpenedWorktree>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeOpenResponse {
    pub opened: Option<OpenedWorktree>,
    pub already_open: Option<bool>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedWorktree {
    pub workspace_id: String,
    pub root_pane_id: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneInfo {
    pub workspace_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCreateResponse {
    pub workspace_id: Option<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaneSplitRequest {
    pub pane_id: String,
    pub direction: OnOpenPaneDirection,
    pub ratio: Option<f32>,
    pub cwd: PathBuf,
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSplitResponse {
    pub pane_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabCreateRequest {
    pub workspace_id: String,
    pub cwd: PathBuf,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabCreateResponse {
    pub root_pane_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneRunResponse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRemoveResponse {
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeOpenTarget {
    Path(PathBuf),
    Branch(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCreateRequest {
    pub cwd: PathBuf,
    pub branch: String,
    pub base: Option<String>,
    pub path: Option<PathBuf>,
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HerdrError {
    UiBusy(String),
    PluginPaneOpenFailed(String),
    WorktreeOperationInProgress(String),
    StaleWorktreeOperation(String),
    LinkedWorktreeSource(String),
    NotGitWorktree(String),
    DirtyWorktreeRequiresForce(String),
    WorktreeCreateFailed(String),
    WorktreeOpenFailed(String),
    WorktreeNotFound(String),
    Other { code: String, message: String },
    InvalidArgument(String),
    Invocation(String),
    InvalidResponse(String),
}

impl fmt::Display for HerdrError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UiBusy(message) => write!(formatter, "ui_busy: {message}"),
            Self::PluginPaneOpenFailed(message) => {
                write!(formatter, "plugin_pane_open_failed: {message}")
            }
            Self::WorktreeOperationInProgress(message) => {
                write!(formatter, "worktree_operation_in_progress: {message}")
            }
            Self::StaleWorktreeOperation(message) => {
                write!(formatter, "stale_worktree_operation: {message}")
            }
            Self::LinkedWorktreeSource(message) => {
                write!(formatter, "linked_worktree_source: {message}")
            }
            Self::NotGitWorktree(message) => write!(formatter, "not_git_worktree: {message}"),
            Self::DirtyWorktreeRequiresForce(message) => {
                write!(formatter, "dirty_worktree_requires_force: {message}")
            }
            Self::WorktreeCreateFailed(message) => {
                write!(formatter, "worktree_create_failed: {message}")
            }
            Self::WorktreeOpenFailed(message) => {
                write!(formatter, "worktree_open_failed: {message}")
            }
            Self::WorktreeNotFound(message) => {
                write!(formatter, "worktree_not_found: {message}")
            }
            Self::Other { code, message } => write!(formatter, "{code}: {message}"),
            Self::InvalidArgument(message) => write!(formatter, "invalid argument: {message}"),
            Self::Invocation(message) => write!(formatter, "herdr invocation failed: {message}"),
            Self::InvalidResponse(message) => {
                write!(formatter, "invalid herdr response: {message}")
            }
        }
    }
}

impl std::error::Error for HerdrError {}

pub trait HerdrProvider: Send + Sync {
    fn worktree_open(
        &self,
        cwd: &Path,
        target: &WorktreeOpenTarget,
        focus: bool,
    ) -> Result<WorktreeOpenResponse, HerdrError>;
    fn worktree_create(
        &self,
        request: &WorktreeCreateRequest,
    ) -> Result<WorktreeCreateResponse, HerdrError>;
    fn worktree_remove(
        &self,
        workspace_id: &str,
        force: bool,
    ) -> Result<WorktreeRemoveResponse, HerdrError>;
    fn worktree_list(&self, cwd: &Path) -> Result<WorktreeListResponse, HerdrError>;
    fn workspace_list(&self) -> Result<Vec<WorkspaceInfo>, HerdrError>;
    fn pane_list(&self) -> Result<Vec<PaneInfo>, HerdrError>;
    fn workspace_create(
        &self,
        cwd: &Path,
        focus: bool,
    ) -> Result<WorkspaceCreateResponse, HerdrError>;
    fn workspace_focus(&self, workspace_id: &str) -> Result<(), HerdrError>;
    fn tab_create(&self, request: &TabCreateRequest) -> Result<TabCreateResponse, HerdrError>;
    fn pane_split(&self, request: &PaneSplitRequest) -> Result<PaneSplitResponse, HerdrError>;
    fn pane_run(&self, pane_id: &str, command: &str) -> Result<PaneRunResponse, HerdrError>;
    fn pane_focus(&self, pane_id: &str) -> Result<(), HerdrError>;
    fn notification_show(&self, title: &str, body: &str) -> Result<(), HerdrError>;
}

#[derive(Debug, Clone)]
pub struct CliHerdrProvider {
    binary_path: PathBuf,
}

impl CliHerdrProvider {
    pub fn new(binary_path: impl Into<PathBuf>) -> Self {
        Self {
            binary_path: binary_path.into(),
        }
    }

    fn invoke<T: DeserializeOwned>(&self, args: &[String]) -> Result<T, HerdrError> {
        let output = self.invoke_output(args)?;
        let envelope: SuccessEnvelope<T> =
            serde_json::from_slice(&output).map_err(|error| invalid_response(&error, &output))?;
        Ok(envelope.result)
    }

    fn invoke_side_effect<T: DeserializeOwned>(
        &self,
        args: &[String],
    ) -> Result<(Option<T>, Option<String>), HerdrError> {
        let output = self.invoke_output(args)?;
        match serde_json::from_slice::<SuccessEnvelope<T>>(&output) {
            Ok(envelope) => Ok((Some(envelope.result), None)),
            Err(error) => Ok((
                None,
                Some(format!(
                    "herdr completed the operation, but returned an unexpected response: {error}"
                )),
            )),
        }
    }

    fn invoke_output(&self, args: &[String]) -> Result<Vec<u8>, HerdrError> {
        let output = Command::new(&self.binary_path)
            .args(args)
            .output()
            .map_err(|error| {
                HerdrError::Invocation(format!(
                    "could not execute {}: {error}",
                    self.binary_path.display()
                ))
            })?;

        if let Some(error) =
            parse_error_envelope(&output.stderr).or_else(|| parse_error_envelope(&output.stdout))
        {
            return Err(error);
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(HerdrError::Invocation(format!(
                "{} exited with {}: {}",
                self.binary_path.display(),
                output.status,
                stderr.trim()
            )));
        }

        Ok(output.stdout)
    }

    fn pane_focus_directional(&self, pane_id: &str) -> Result<(), HerdrError> {
        let args = vec![
            "pane".into(),
            "layout".into(),
            "--pane".into(),
            pane_id.into(),
        ];
        let PaneLayoutResult::PaneLayout { layout } = self.invoke(&args)?;
        self.invoke_output(&["tab".into(), "focus".into(), layout.tab_id.clone()])?;

        let mut focused_pane_id = layout.focused_pane_id;
        let target = layout
            .panes
            .iter()
            .find(|pane| pane.pane_id == pane_id)
            .ok_or_else(|| {
                HerdrError::InvalidResponse(format!(
                    "pane layout did not contain focus target {pane_id}"
                ))
            })?;
        for _ in 0..layout.panes.len().saturating_mul(2).max(1) {
            if focused_pane_id == pane_id {
                return Ok(());
            }
            let focused = layout
                .panes
                .iter()
                .find(|pane| pane.pane_id == focused_pane_id)
                .ok_or_else(|| {
                    HerdrError::InvalidResponse(format!(
                        "pane layout did not contain focused pane {focused_pane_id}"
                    ))
                })?;
            let direction = direction_toward(focused.rect, target.rect);
            let args = vec![
                "pane".into(),
                "focus".into(),
                "--direction".into(),
                direction.into(),
                "--pane".into(),
                focused_pane_id.clone(),
            ];
            let PaneFocusDirectionResult::PaneFocusDirection { focus } = self.invoke(&args)?;
            let Some(next_pane_id) = focus.focused_pane_id else {
                break;
            };
            if next_pane_id == focused_pane_id {
                break;
            }
            focused_pane_id = next_pane_id;
        }
        Err(HerdrError::InvalidResponse(format!(
            "directional pane focus could not reach {pane_id}"
        )))
    }
}

impl HerdrProvider for CliHerdrProvider {
    fn worktree_open(
        &self,
        cwd: &Path,
        target: &WorktreeOpenTarget,
        focus: bool,
    ) -> Result<WorktreeOpenResponse, HerdrError> {
        require_absolute("cwd", cwd)?;
        let mut args = vec![
            "worktree".into(),
            "open".into(),
            "--cwd".into(),
            path_arg("cwd", cwd)?,
        ];
        match target {
            WorktreeOpenTarget::Path(path) => {
                require_absolute("worktree path", path)?;
                args.extend(["--path".into(), path_arg("worktree path", path)?]);
            }
            WorktreeOpenTarget::Branch(branch) => {
                require_nonempty("branch", branch)?;
                args.extend(["--branch".into(), branch.clone()]);
            }
        }
        args.push(if focus { "--focus" } else { "--no-focus" }.into());

        let (result, warning) = self.invoke_side_effect::<WorktreeOpenResult>(&args)?;
        let (opened, already_open) = result.map_or((None, None), |result| match result {
            WorktreeOpenResult::WorktreeOpened {
                workspace,
                root_pane,
                worktree,
                already_open,
            } => (
                Some(OpenedWorktree {
                    workspace_id: workspace.workspace_id,
                    root_pane_id: root_pane.pane_id,
                    path: worktree.path,
                }),
                Some(already_open),
            ),
        });
        Ok(WorktreeOpenResponse {
            opened,
            already_open,
            warning,
        })
    }

    fn worktree_create(
        &self,
        request: &WorktreeCreateRequest,
    ) -> Result<WorktreeCreateResponse, HerdrError> {
        require_absolute("cwd", &request.cwd)?;
        require_nonempty("branch", &request.branch)?;
        let mut args = vec![
            "worktree".into(),
            "create".into(),
            "--cwd".into(),
            path_arg("cwd", &request.cwd)?,
            "--branch".into(),
            request.branch.clone(),
        ];
        if let Some(base) = &request.base {
            require_nonempty("base", base)?;
            args.extend(["--base".into(), base.clone()]);
        }
        if let Some(path) = &request.path {
            require_absolute("worktree path", path)?;
            args.extend(["--path".into(), path_arg("worktree path", path)?]);
        }
        args.push(
            if request.focus {
                "--focus"
            } else {
                "--no-focus"
            }
            .into(),
        );

        let (result, warning) = self.invoke_side_effect::<WorktreeCreateResult>(&args)?;
        let opened = result.map(|result| match result {
            WorktreeCreateResult::WorktreeCreated {
                workspace,
                root_pane,
                worktree,
            } => OpenedWorktree {
                workspace_id: workspace.workspace_id,
                root_pane_id: root_pane.pane_id,
                path: worktree.path,
            },
        });
        Ok(WorktreeCreateResponse { opened, warning })
    }

    fn worktree_remove(
        &self,
        workspace_id: &str,
        force: bool,
    ) -> Result<WorktreeRemoveResponse, HerdrError> {
        require_nonempty("workspace id", workspace_id)?;
        let mut args = vec![
            "worktree".into(),
            "remove".into(),
            "--workspace".into(),
            workspace_id.into(),
        ];
        if force {
            args.push("--force".into());
        }
        let (_, warning) = self.invoke_side_effect::<WorktreeRemoveResult>(&args)?;
        Ok(WorktreeRemoveResponse { warning })
    }

    fn worktree_list(&self, cwd: &Path) -> Result<WorktreeListResponse, HerdrError> {
        require_absolute("cwd", cwd)?;
        let args = vec![
            "worktree".into(),
            "list".into(),
            "--cwd".into(),
            path_arg("cwd", cwd)?,
            "--json".into(),
        ];
        match self.invoke::<WorktreeListResult>(&args)? {
            WorktreeListResult::WorktreeList { worktrees } => {
                Ok(WorktreeListResponse { worktrees })
            }
        }
    }

    fn workspace_list(&self) -> Result<Vec<WorkspaceInfo>, HerdrError> {
        let args = vec!["workspace".into(), "list".into()];
        match self.invoke::<WorkspaceListResult>(&args)? {
            WorkspaceListResult::WorkspaceList { workspaces } => Ok(workspaces),
        }
    }

    fn pane_list(&self) -> Result<Vec<PaneInfo>, HerdrError> {
        let args = vec!["pane".into(), "list".into()];
        match self.invoke::<PaneListResult>(&args)? {
            PaneListResult::PaneList { panes } => Ok(panes),
        }
    }

    fn workspace_create(
        &self,
        cwd: &Path,
        focus: bool,
    ) -> Result<WorkspaceCreateResponse, HerdrError> {
        require_absolute("cwd", cwd)?;
        let mut args = vec![
            "workspace".into(),
            "create".into(),
            "--cwd".into(),
            path_arg("cwd", cwd)?,
        ];
        args.push(if focus { "--focus" } else { "--no-focus" }.into());
        let (result, warning) = self.invoke_side_effect::<WorkspaceCreateResult>(&args)?;
        Ok(WorkspaceCreateResponse {
            workspace_id: result.map(|result| match result {
                WorkspaceCreateResult::WorkspaceCreated { workspace } => workspace.workspace_id,
            }),
            warning,
        })
    }

    fn workspace_focus(&self, workspace_id: &str) -> Result<(), HerdrError> {
        require_nonempty("workspace id", workspace_id)?;
        self.invoke_output(&["workspace".into(), "focus".into(), workspace_id.into()])?;
        Ok(())
    }

    fn tab_create(&self, request: &TabCreateRequest) -> Result<TabCreateResponse, HerdrError> {
        require_nonempty("workspace id", &request.workspace_id)?;
        require_absolute("cwd", &request.cwd)?;
        let mut args = vec![
            "tab".into(),
            "create".into(),
            "--workspace".into(),
            request.workspace_id.clone(),
            "--cwd".into(),
            path_arg("cwd", &request.cwd)?,
        ];
        if let Some(label) = &request.label {
            require_nonempty("tab label", label)?;
            args.extend(["--label".into(), label.clone()]);
        }
        args.push("--no-focus".into());
        match self.invoke::<TabCreateResult>(&args)? {
            TabCreateResult::TabCreated { root_pane } => Ok(TabCreateResponse {
                root_pane_id: root_pane.pane_id,
            }),
        }
    }

    fn pane_split(&self, request: &PaneSplitRequest) -> Result<PaneSplitResponse, HerdrError> {
        require_nonempty("pane id", &request.pane_id)?;
        require_absolute("cwd", &request.cwd)?;
        if request
            .ratio
            .is_some_and(|ratio| !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0)
        {
            return Err(HerdrError::InvalidArgument(
                "pane split ratio must be greater than 0 and less than 1".into(),
            ));
        }
        let mut args = vec![
            "pane".into(),
            "split".into(),
            request.pane_id.clone(),
            "--direction".into(),
            request.direction.as_str().into(),
        ];
        if let Some(ratio) = request.ratio {
            args.extend(["--ratio".into(), ratio.to_string()]);
        }
        args.extend([
            "--cwd".into(),
            path_arg("cwd", &request.cwd)?,
            if request.focus {
                "--focus"
            } else {
                "--no-focus"
            }
            .into(),
        ]);
        match self.invoke::<PaneSplitResult>(&args)? {
            PaneSplitResult::PaneInfo { pane } => Ok(PaneSplitResponse {
                pane_id: pane.pane_id,
            }),
        }
    }

    fn pane_run(&self, pane_id: &str, command: &str) -> Result<PaneRunResponse, HerdrError> {
        require_nonempty("pane id", pane_id)?;
        require_nonempty("command", command)?;
        let args = vec!["pane".into(), "run".into(), pane_id.into(), command.into()];
        self.invoke_output(&args)?;
        Ok(PaneRunResponse)
    }

    fn pane_focus(&self, pane_id: &str) -> Result<(), HerdrError> {
        require_nonempty("pane id", pane_id)?;
        match self.invoke_output(&["pane".into(), "focus".into(), pane_id.into()]) {
            Ok(_) => Ok(()),
            Err(HerdrError::Invocation(message))
                if message.contains("pane focus") && message.contains("--direction") =>
            {
                self.pane_focus_directional(pane_id)
            }
            Err(error) => Err(error),
        }
    }

    fn notification_show(&self, title: &str, body: &str) -> Result<(), HerdrError> {
        require_nonempty("notification title", title)?;
        require_nonempty("notification body", body)?;
        self.invoke_output(&[
            "notification".into(),
            "show".into(),
            title.into(),
            "--body".into(),
            body.into(),
        ])?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct SuccessEnvelope<T> {
    result: T,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    code: String,
    message: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkspaceListResult {
    WorkspaceList { workspaces: Vec<WorkspaceInfo> },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PaneListResult {
    PaneList { panes: Vec<PaneInfo> },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkspaceCreateResult {
    WorkspaceCreated { workspace: WorkspaceIdentity },
}

#[derive(Deserialize)]
struct WorkspaceIdentity {
    workspace_id: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorktreeListResult {
    WorktreeList { worktrees: Vec<WorktreeInfo> },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorktreeCreateResult {
    WorktreeCreated {
        workspace: WorkspaceIdentity,
        root_pane: RootPaneInfo,
        worktree: WorktreePath,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorktreeOpenResult {
    WorktreeOpened {
        workspace: WorkspaceIdentity,
        root_pane: RootPaneInfo,
        worktree: WorktreePath,
        already_open: bool,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TabCreateResult {
    TabCreated { root_pane: RootPaneInfo },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PaneSplitResult {
    PaneInfo { pane: RootPaneInfo },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PaneLayoutResult {
    PaneLayout { layout: PaneLayoutInfo },
}

#[derive(Deserialize)]
struct PaneLayoutInfo {
    tab_id: String,
    focused_pane_id: String,
    panes: Vec<PaneLayoutPaneInfo>,
}

#[derive(Deserialize)]
struct PaneLayoutPaneInfo {
    pane_id: String,
    rect: PaneLayoutRect,
}

#[derive(Clone, Copy, Deserialize)]
struct PaneLayoutRect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PaneFocusDirectionResult {
    PaneFocusDirection { focus: PaneFocusDirectionInfo },
}

#[derive(Deserialize)]
struct PaneFocusDirectionInfo {
    focused_pane_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorktreeRemoveResult {
    WorktreeRemoved {},
}

#[derive(Deserialize)]
struct WorktreePath {
    path: String,
}

#[derive(Deserialize)]
struct RootPaneInfo {
    pane_id: String,
}

fn require_absolute(name: &str, path: &Path) -> Result<(), HerdrError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(HerdrError::InvalidArgument(format!(
            "{name} must be absolute: {}",
            path.display()
        )))
    }
}

fn require_nonempty(name: &str, value: &str) -> Result<(), HerdrError> {
    if value.trim().is_empty() {
        Err(HerdrError::InvalidArgument(format!(
            "{name} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn path_arg(name: &str, path: &Path) -> Result<String, HerdrError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        HerdrError::InvalidArgument(format!(
            "{name} is not valid UTF-8 and cannot be passed to herdr"
        ))
    })
}

fn direction_toward(source: PaneLayoutRect, target: PaneLayoutRect) -> &'static str {
    if target.x >= source.x.saturating_add(source.width) {
        "right"
    } else if source.x >= target.x.saturating_add(target.width) {
        "left"
    } else if target.y >= source.y.saturating_add(source.height) {
        "down"
    } else {
        "up"
    }
}

fn invalid_response(error: &serde_json::Error, output: &[u8]) -> HerdrError {
    HerdrError::InvalidResponse(format!(
        "{error}; stdout was {:?}",
        String::from_utf8_lossy(output).trim()
    ))
}

fn parse_error_envelope(bytes: &[u8]) -> Option<HerdrError> {
    let envelope: ErrorEnvelope = serde_json::from_slice(bytes).ok()?;
    let ErrorBody { code, message } = envelope.error;
    Some(match code.as_str() {
        "ui_busy" => HerdrError::UiBusy(message),
        "plugin_pane_open_failed" => HerdrError::PluginPaneOpenFailed(message),
        "worktree_operation_in_progress" => HerdrError::WorktreeOperationInProgress(message),
        "stale_worktree_operation" => HerdrError::StaleWorktreeOperation(message),
        "linked_worktree_source" => HerdrError::LinkedWorktreeSource(message),
        "not_git_worktree" => HerdrError::NotGitWorktree(message),
        "dirty_worktree_requires_force" => HerdrError::DirtyWorktreeRequiresForce(message),
        "worktree_create_failed" => HerdrError::WorktreeCreateFailed(message),
        "worktree_open_failed" => HerdrError::WorktreeOpenFailed(message),
        "worktree_not_found" => HerdrError::WorktreeNotFound(message),
        _ => HerdrError::Other { code, message },
    })
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::{fs, os::unix::fs::PermissionsExt};

    #[cfg(unix)]
    use tempfile::TempDir;

    use super::*;

    const WORKSPACE: &str = r#"{
        "workspace_id":"w_1","number":1,"label":"repo","focused":true,
        "pane_count":1,"tab_count":1,"active_tab_id":"w_1:1",
        "agent_status":"unknown","tokens":{}
    }"#;

    const WORKSPACE_WITH_WORKTREE: &str = r#"{
        "workspace_id":"w_2","number":2,"label":"repo feature","focused":false,
        "pane_count":1,"tab_count":1,"active_tab_id":"w_2:1",
        "agent_status":"idle","tokens":{"review":"ready"},
        "worktree":{"repo_key":"/repo/.git","repo_name":"repo","repo_root":"/repo",
        "checkout_path":"/worktrees/repo/feature","is_linked_worktree":true}
    }"#;

    const WORKTREE_OPEN: &str = r#"{
        "path":"/worktrees/repo/feature","branch":"feature","is_bare":false,
        "is_detached":false,"is_prunable":false,"is_linked_worktree":true,
        "open_workspace_id":"w_2","label":"repo feature"
    }"#;

    const ROOT_PANE: &str = r#"{"pane_id":"p_root"}"#;

    const WORKTREE_CLOSED: &str = r#"{
        "path":"/repo","branch":"main","is_bare":false,"is_detached":false,
        "is_prunable":false,"is_linked_worktree":false,"label":"repo"
    }"#;

    fn parse_success<T: DeserializeOwned>(json: &str) -> T {
        serde_json::from_str::<SuccessEnvelope<T>>(json)
            .unwrap()
            .result
    }

    #[test]
    fn parses_realistic_workspace_list_with_optional_worktree() {
        let json = format!(
            r#"{{"id":"cli:workspace:list","result":{{"type":"workspace_list","workspaces":[{WORKSPACE},{WORKSPACE_WITH_WORKTREE}]}}}}"#
        );
        let WorkspaceListResult::WorkspaceList { workspaces } = parse_success(&json);
        assert!(workspaces[0].worktree.is_none());
        assert_eq!(
            workspaces[1]
                .worktree
                .as_ref()
                .map(|worktree| worktree.repo_root.as_str()),
            Some("/repo")
        );
    }

    #[test]
    fn parses_pane_list_with_only_folder_identity_fields() {
        let json = r#"{"result":{"type":"pane_list","panes":[
            {"pane_id":"p_1","workspace_id":"w_1","cwd":"/folder","foreground_cwd":"/folder/subdir"},
            {"pane_id":"p_2","workspace_id":"w_2"}
        ]}}"#;
        let PaneListResult::PaneList { panes } = parse_success(json);
        assert_eq!(panes[0].workspace_id, "w_1");
        assert_eq!(panes[0].cwd.as_deref(), Some("/folder"));
        assert_eq!(panes[0].foreground_cwd.as_deref(), Some("/folder/subdir"));
        assert!(panes[1].cwd.is_none());
    }

    #[test]
    fn parses_realistic_worktree_list_with_open_and_closed_entries() {
        let json = format!(
            r#"{{"id":"cli:worktree:list","result":{{"type":"worktree_list","source":{{"repo_key":"/repo/.git","repo_name":"repo","repo_root":"/repo","source_checkout_path":"/repo"}},"worktrees":[{WORKTREE_CLOSED},{WORKTREE_OPEN}]}}}}"#
        );
        let WorktreeListResult::WorktreeList { worktrees } = parse_success(&json);
        assert!(worktrees[0].open_workspace_id.is_none());
        assert_eq!(worktrees[1].open_workspace_id.as_deref(), Some("w_2"));
    }

    #[test]
    fn parses_create_open_and_remove_response_shapes() {
        let created = format!(
            r#"{{"id":"create","result":{{"type":"worktree_created","workspace":{WORKSPACE_WITH_WORKTREE},"tab":{{}},"root_pane":{ROOT_PANE},"worktree":{WORKTREE_OPEN}}}}}"#
        );
        let WorktreeCreateResult::WorktreeCreated { worktree, .. } = parse_success(&created);
        assert_eq!(worktree.path, "/worktrees/repo/feature");

        let opened = format!(
            r#"{{"id":"open","result":{{"type":"worktree_opened","workspace":{WORKSPACE_WITH_WORKTREE},"tab":{{}},"root_pane":{ROOT_PANE},"worktree":{WORKTREE_OPEN},"already_open":true}}}}"#
        );
        let WorktreeOpenResult::WorktreeOpened { already_open, .. } = parse_success(&opened);
        assert!(already_open);

        let removed = r#"{"id":"remove","result":{"type":"worktree_removed","workspace_id":"w_2","path":"/worktrees/repo/feature","forced":true}}"#;
        assert!(matches!(
            parse_success(removed),
            WorktreeRemoveResult::WorktreeRemoved {}
        ));
    }

    #[test]
    fn maps_all_required_error_codes_and_preserves_unknown_errors() {
        let cases = [
            ("ui_busy", "UiBusy"),
            ("plugin_pane_open_failed", "PluginPaneOpenFailed"),
            (
                "worktree_operation_in_progress",
                "WorktreeOperationInProgress",
            ),
            ("stale_worktree_operation", "StaleWorktreeOperation"),
            ("linked_worktree_source", "LinkedWorktreeSource"),
            ("not_git_worktree", "NotGitWorktree"),
            (
                "dirty_worktree_requires_force",
                "DirtyWorktreeRequiresForce",
            ),
            ("worktree_create_failed", "WorktreeCreateFailed"),
            ("worktree_open_failed", "WorktreeOpenFailed"),
            ("worktree_not_found", "WorktreeNotFound"),
        ];
        for (code, expected_variant) in cases {
            let envelope =
                format!(r#"{{"id":"error","error":{{"code":"{code}","message":"details"}}}}"#);
            let error = parse_error_envelope(envelope.as_bytes()).unwrap();
            assert!(
                format!("{error:?}").starts_with(expected_variant),
                "wrong variant for {code}: {error:?}"
            );
        }

        let other = parse_error_envelope(
            br#"{"id":"error","error":{"code":"future_error","message":"details"}}"#,
        )
        .unwrap();
        assert_eq!(
            other,
            HerdrError::Other {
                code: "future_error".into(),
                message: "details".into()
            }
        );
    }

    #[cfg(unix)]
    fn fake_herdr(temp: &TempDir, response: &str) -> (PathBuf, PathBuf) {
        fake_herdr_with_output(temp, response, "", 0)
    }

    #[cfg(unix)]
    fn fake_herdr_with_output(
        temp: &TempDir,
        stdout: &str,
        stderr: &str,
        exit_status: u8,
    ) -> (PathBuf, PathBuf) {
        let binary = temp.path().join("fake-herdr");
        let args_file = temp.path().join("args");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\nprintf '%s' '{}'\nprintf '%s' '{}' >&2\nexit {exit_status}\n",
            args_file.display(),
            stdout,
            stderr
        );
        fs::write(&binary, script).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        (binary, args_file)
    }

    #[cfg(unix)]
    fn retry_fake_herdr<T>(mut invoke: impl FnMut() -> Result<T, HerdrError>) -> T {
        const ATTEMPTS: usize = 100;
        for attempt in 1..=ATTEMPTS {
            match invoke() {
                Ok(value) => return value,
                Err(HerdrError::Invocation(message))
                    if attempt < ATTEMPTS
                        && (message.contains("Text file busy")
                            || message.contains("os error 26")) =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(error) => panic!("fake herdr invocation failed: {error}"),
            }
        }
        unreachable!()
    }

    #[cfg(unix)]
    #[test]
    fn cli_uses_workspace_list_without_json_flag() {
        let temp = TempDir::new().unwrap();
        let response = format!(
            r#"{{"id":"list","result":{{"type":"workspace_list","workspaces":[{WORKSPACE}]}}}}"#
        );
        let (binary, args_file) = fake_herdr(&temp, &response);
        let provider = CliHerdrProvider::new(binary);
        assert_eq!(retry_fake_herdr(|| provider.workspace_list()).len(), 1);
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "workspace list"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cli_uses_unscoped_pane_list() {
        let temp = TempDir::new().unwrap();
        let response =
            r#"{"result":{"type":"pane_list","panes":[{"workspace_id":"w_1","cwd":"/folder"}]}}"#;
        let (binary, args_file) = fake_herdr(&temp, response);
        let provider = CliHerdrProvider::new(binary);
        let panes = retry_fake_herdr(|| provider.pane_list());
        assert_eq!(panes[0].cwd.as_deref(), Some("/folder"));
        assert_eq!(fs::read_to_string(args_file).unwrap().trim(), "pane list");
    }

    #[cfg(unix)]
    #[test]
    fn cli_uses_json_flag_only_for_worktree_list() {
        let temp = TempDir::new().unwrap();
        let response = format!(
            r#"{{"id":"list","result":{{"type":"worktree_list","source":{{"repo_key":"/repo/.git","repo_name":"repo","repo_root":"/repo","source_checkout_path":"/repo"}},"worktrees":[{WORKTREE_CLOSED}]}}}}"#
        );
        let (binary, args_file) = fake_herdr(&temp, &response);
        let provider = CliHerdrProvider::new(binary);
        retry_fake_herdr(|| provider.worktree_list(Path::new("/repo")));
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "worktree list --cwd /repo --json"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cli_builds_exact_open_calls_for_path_and_branch() {
        let response = format!(
            r#"{{"id":"open","result":{{"type":"worktree_opened","workspace":{WORKSPACE_WITH_WORKTREE},"root_pane":{ROOT_PANE},"worktree":{WORKTREE_OPEN},"already_open":false}}}}"#
        );

        let path_temp = TempDir::new().unwrap();
        let (binary, args_file) = fake_herdr(&path_temp, &response);
        let provider = CliHerdrProvider::new(binary);
        retry_fake_herdr(|| {
            provider.worktree_open(
                Path::new("/repo"),
                &WorktreeOpenTarget::Path("/repo".into()),
                true,
            )
        });
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "worktree open --cwd /repo --path /repo --focus"
        );

        let branch_temp = TempDir::new().unwrap();
        let (binary, args_file) = fake_herdr(&branch_temp, &response);
        let provider = CliHerdrProvider::new(binary);
        retry_fake_herdr(|| {
            provider.worktree_open(
                Path::new("/repo"),
                &WorktreeOpenTarget::Branch("feature".into()),
                true,
            )
        });
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "worktree open --cwd /repo --branch feature --focus"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cli_builds_exact_create_and_remove_calls() {
        let create_response = format!(
            r#"{{"id":"create","result":{{"type":"worktree_created","workspace":{WORKSPACE_WITH_WORKTREE},"root_pane":{ROOT_PANE},"worktree":{WORKTREE_OPEN}}}}}"#
        );
        let create_temp = TempDir::new().unwrap();
        let (binary, args_file) = fake_herdr(&create_temp, &create_response);
        let provider = CliHerdrProvider::new(binary);
        retry_fake_herdr(|| {
            provider.worktree_create(&WorktreeCreateRequest {
                cwd: "/repo".into(),
                branch: "feature".into(),
                base: Some("main".into()),
                path: Some("/custom/repo-feature".into()),
                focus: true,
            })
        });
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "worktree create --cwd /repo --branch feature --base main --path /custom/repo-feature --focus"
        );

        let remove_response = r#"{"id":"remove","result":{"type":"worktree_removed","workspace_id":"w_2","path":"/worktrees/repo/feature","forced":true}}"#;
        let remove_temp = TempDir::new().unwrap();
        let (binary, args_file) = fake_herdr(&remove_temp, remove_response);
        let provider = CliHerdrProvider::new(binary);
        retry_fake_herdr(|| provider.worktree_remove("w_2", true));
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "worktree remove --workspace w_2 --force"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cli_builds_exact_workspace_create_and_focus_calls() {
        let create_temp = TempDir::new().unwrap();
        let create_response = format!(
            r#"{{"result":{{"type":"workspace_created","workspace":{WORKSPACE},"tab":{{}},"root_pane":{ROOT_PANE}}}}}"#
        );
        let (binary, args_file) = fake_herdr(&create_temp, &create_response);
        let provider = CliHerdrProvider::new(binary);
        let response =
            retry_fake_herdr(|| provider.workspace_create(Path::new("/plain folder"), true));
        assert_eq!(response.workspace_id.as_deref(), Some("w_1"));
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "workspace create --cwd /plain folder --focus"
        );

        let focus_temp = TempDir::new().unwrap();
        let (binary, args_file) = fake_herdr(
            &focus_temp,
            r#"{"result":{"type":"workspace_info","workspace":{"workspace_id":"w_1"}}}"#,
        );
        let provider = CliHerdrProvider::new(binary);
        retry_fake_herdr(|| provider.workspace_focus("w_1"));
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "workspace focus w_1"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cli_builds_exact_tab_create_calls() {
        let response =
            r#"{"result":{"type":"tab_created","tab":{},"root_pane":{"pane_id":"p_tab"}}}"#;
        let labeled_temp = TempDir::new().unwrap();
        let (binary, args_file) = fake_herdr(&labeled_temp, response);
        let provider = CliHerdrProvider::new(binary);
        let created = retry_fake_herdr(|| {
            provider.tab_create(&TabCreateRequest {
                workspace_id: "w_1".into(),
                cwd: "/repo checkout".into(),
                label: Some("server".into()),
            })
        });
        assert_eq!(created.root_pane_id, "p_tab");
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "tab create --workspace w_1 --cwd /repo checkout --label server --no-focus"
        );

        let unlabeled_temp = TempDir::new().unwrap();
        let (binary, args_file) = fake_herdr(&unlabeled_temp, response);
        let provider = CliHerdrProvider::new(binary);
        retry_fake_herdr(|| {
            provider.tab_create(&TabCreateRequest {
                workspace_id: "w_1".into(),
                cwd: "/repo".into(),
                label: None,
            })
        });
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "tab create --workspace w_1 --cwd /repo --no-focus"
        );
    }

    #[cfg(unix)]
    #[test]
    fn side_effect_responses_ignore_unused_and_future_fields() {
        let temp = TempDir::new().unwrap();
        let response = r#"{"result":{"type":"worktree_opened","workspace":{"workspace_id":"w_1","future_workspace_field":1},"root_pane":{"pane_id":"p_root","future_pane_field":1},"worktree":{"path":"/repo","renamed_label":"repo"},"already_open":false,"future_result_field":true}}"#;
        let (binary, _) = fake_herdr(&temp, response);
        let provider = CliHerdrProvider::new(binary);

        let opened = retry_fake_herdr(|| {
            provider.worktree_open(
                Path::new("/repo"),
                &WorktreeOpenTarget::Path("/repo".into()),
                true,
            )
        });

        let opened_worktree = opened.opened.unwrap();
        assert_eq!(opened_worktree.workspace_id, "w_1");
        assert_eq!(opened_worktree.path, "/repo");
        assert_eq!(opened.already_open, Some(false));
        assert!(opened.warning.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn undecodable_success_is_soft_success_but_error_envelope_stays_failure() {
        let success_temp = TempDir::new().unwrap();
        let (binary, _) = fake_herdr(
            &success_temp,
            r#"{"result":{"type":"worktree_created","workspace_id":"w_1"}}"#,
        );
        let provider = CliHerdrProvider::new(binary);
        let response = retry_fake_herdr(|| {
            provider.worktree_create(&WorktreeCreateRequest {
                cwd: "/repo".into(),
                branch: "feature".into(),
                base: None,
                path: None,
                focus: true,
            })
        });
        assert!(response.opened.is_none());
        assert!(response.warning.as_deref().is_some_and(|warning| {
            warning.contains("completed the operation") && warning.contains("unexpected response")
        }));

        let error_temp = TempDir::new().unwrap();
        let (binary, _) = fake_herdr(
            &error_temp,
            r#"{"error":{"code":"worktree_create_failed","message":"disk full"}}"#,
        );
        let provider = CliHerdrProvider::new(binary);
        let error = provider
            .worktree_create(&WorktreeCreateRequest {
                cwd: "/repo".into(),
                branch: "feature".into(),
                base: None,
                path: None,
                focus: true,
            })
            .unwrap_err();
        assert_eq!(error, HerdrError::WorktreeCreateFailed("disk full".into()));
    }

    #[cfg(unix)]
    #[test]
    fn remove_succeeds_without_now_unused_response_fields() {
        let temp = TempDir::new().unwrap();
        let (binary, _) = fake_herdr(
            &temp,
            r#"{"result":{"type":"worktree_removed","future_field":true}}"#,
        );
        let provider = CliHerdrProvider::new(binary);

        let response = retry_fake_herdr(|| provider.worktree_remove("w_1", false));
        assert!(response.warning.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn cli_builds_exact_notification_call_without_decoding_its_response() {
        let temp = TempDir::new().unwrap();
        let (binary, args_file) = fake_herdr(&temp, "future non-json response");
        let provider = CliHerdrProvider::new(binary);

        retry_fake_herdr(|| provider.notification_show("herdr-kiosk", "on_open failed"));
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "notification show herdr-kiosk --body on_open failed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cli_builds_exact_pane_split_call() {
        let split_temp = TempDir::new().unwrap();
        let (binary, args_file) = fake_herdr(
            &split_temp,
            r#"{"id":"split","result":{"type":"pane_info","pane":{"pane_id":"p_2"}}}"#,
        );
        let provider = CliHerdrProvider::new(binary);
        let response = retry_fake_herdr(|| {
            provider.pane_split(&PaneSplitRequest {
                pane_id: "p_root".into(),
                direction: OnOpenPaneDirection::Right,
                ratio: Some(0.4),
                cwd: "/repo checkout".into(),
                focus: false,
            })
        });
        assert_eq!(response.pane_id, "p_2");
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "pane split p_root --direction right --ratio 0.4 --cwd /repo checkout --no-focus"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cli_pane_run_accepts_empty_stdout_and_surfaces_error_envelopes() {
        let run_temp = TempDir::new().unwrap();
        let (binary, args_file) = fake_herdr_with_output(&run_temp, "", "", 0);
        let provider = CliHerdrProvider::new(binary);
        retry_fake_herdr(|| provider.pane_run("p_2", "printf ONOPEN_OK"));
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "pane run p_2 printf ONOPEN_OK"
        );

        let error_temp = TempDir::new().unwrap();
        let (binary, _) = fake_herdr_with_output(
            &error_temp,
            "",
            r#"{"error":{"code":"pane_run_failed","message":"pane unavailable"}}"#,
            1,
        );
        let provider = CliHerdrProvider::new(binary);
        assert_eq!(
            provider.pane_run("p_2", "printf ONOPEN_OK"),
            Err(HerdrError::Other {
                code: "pane_run_failed".into(),
                message: "pane unavailable".into(),
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn cli_pane_focus_accepts_empty_stdout() {
        let temp = TempDir::new().unwrap();
        let (binary, args_file) = fake_herdr_with_output(&temp, "", "", 0);
        let provider = CliHerdrProvider::new(binary);

        retry_fake_herdr(|| provider.pane_focus("p_2"));
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "pane focus p_2"
        );
    }

    #[test]
    fn cli_rejects_relative_paths_before_invocation() {
        let provider = CliHerdrProvider::new("/does/not/exist");
        assert!(matches!(
            provider.worktree_list(Path::new("relative")),
            Err(HerdrError::InvalidArgument(_))
        ));
        assert!(matches!(
            provider.workspace_create(Path::new("relative"), true),
            Err(HerdrError::InvalidArgument(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn cli_rejects_non_utf8_paths_before_invocation() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let temp = TempDir::new().unwrap();
        let response = format!(
            r#"{{"id":"open","result":{{"type":"worktree_opened","workspace":{WORKSPACE_WITH_WORKTREE},"root_pane":{ROOT_PANE},"worktree":{WORKTREE_OPEN},"already_open":false}}}}"#
        );
        let (binary, args_file) = fake_herdr(&temp, &response);
        let provider = CliHerdrProvider::new(binary);
        let path = PathBuf::from(OsString::from_vec(b"/repo/invalid-\xff".to_vec()));

        let error = provider
            .worktree_create(&WorktreeCreateRequest {
                cwd: path,
                branch: "feature".into(),
                base: None,
                path: None,
                focus: true,
            })
            .unwrap_err();

        assert!(
            matches!(error, HerdrError::InvalidArgument(message) if message.contains("not valid UTF-8"))
        );
        assert!(!args_file.exists(), "herdr must not be invoked");

        let error = provider
            .pane_split(&PaneSplitRequest {
                pane_id: "p_root".into(),
                direction: OnOpenPaneDirection::Right,
                ratio: None,
                cwd: PathBuf::from(OsString::from_vec(b"/repo/invalid-\xff".to_vec())),
                focus: false,
            })
            .unwrap_err();
        assert!(
            matches!(error, HerdrError::InvalidArgument(message) if message.contains("not valid UTF-8"))
        );
        assert!(!args_file.exists(), "herdr must not be invoked");
    }
}
