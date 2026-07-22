use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, de::DeserializeOwned};

use crate::config::OnOpenPaneDirection;

pub mod mock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceWorktreeInfo {
    pub repo_key: String,
    pub repo_name: String,
    pub repo_root: String,
    pub checkout_path: String,
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub focused: bool,
    pub pane_count: usize,
    pub tab_count: usize,
    pub active_tab_id: String,
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub tokens: HashMap<String, String>,
    #[serde(default)]
    pub worktree: Option<WorkspaceWorktreeInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorktreeSourceInfo {
    pub repo_key: String,
    pub repo_name: String,
    pub repo_root: String,
    pub source_checkout_path: String,
    #[serde(default)]
    pub source_workspace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct WorktreeInfo {
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
    pub is_bare: bool,
    pub is_detached: bool,
    pub is_prunable: bool,
    pub is_linked_worktree: bool,
    #[serde(default)]
    pub open_workspace_id: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeListResponse {
    pub source: WorktreeSourceInfo,
    pub worktrees: Vec<WorktreeInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCreateResponse {
    pub workspace: WorkspaceInfo,
    pub root_pane: PaneInfo,
    pub worktree: WorktreeInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeOpenResponse {
    pub workspace: WorkspaceInfo,
    pub root_pane: PaneInfo,
    pub worktree: WorktreeInfo,
    pub already_open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneInfo {
    pub pane_id: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneRunResponse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRemoveResponse {
    pub workspace_id: String,
    pub path: String,
    pub forced: bool,
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
    fn pane_split(&self, request: &PaneSplitRequest) -> Result<PaneSplitResponse, HerdrError>;
    fn pane_run(&self, pane_id: &str, command: &str) -> Result<PaneRunResponse, HerdrError>;
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

        let envelope: SuccessEnvelope<T> =
            serde_json::from_slice(&output.stdout).map_err(|error| {
                HerdrError::InvalidResponse(format!(
                    "{error}; stdout was {:?}",
                    String::from_utf8_lossy(&output.stdout).trim()
                ))
            })?;
        Ok(envelope.result)
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

        match self.invoke::<WorktreeOpenResult>(&args)? {
            WorktreeOpenResult::WorktreeOpened {
                workspace,
                root_pane,
                worktree,
                already_open,
            } => Ok(WorktreeOpenResponse {
                workspace,
                root_pane,
                worktree,
                already_open,
            }),
        }
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

        match self.invoke::<WorktreeCreateResult>(&args)? {
            WorktreeCreateResult::WorktreeCreated {
                workspace,
                root_pane,
                worktree,
            } => Ok(WorktreeCreateResponse {
                workspace,
                root_pane,
                worktree,
            }),
        }
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
        match self.invoke::<WorktreeRemoveResult>(&args)? {
            WorktreeRemoveResult::WorktreeRemoved {
                workspace_id,
                path,
                forced,
            } => Ok(WorktreeRemoveResponse {
                workspace_id,
                path,
                forced,
            }),
        }
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
            WorktreeListResult::WorktreeList { source, worktrees } => {
                Ok(WorktreeListResponse { source, worktrees })
            }
        }
    }

    fn workspace_list(&self) -> Result<Vec<WorkspaceInfo>, HerdrError> {
        let args = vec!["workspace".into(), "list".into()];
        match self.invoke::<WorkspaceListResult>(&args)? {
            WorkspaceListResult::WorkspaceList { workspaces } => Ok(workspaces),
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
        match self.invoke::<PaneRunResult>(&args)? {
            PaneRunResult::Ok {} => Ok(PaneRunResponse),
        }
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
enum WorktreeListResult {
    WorktreeList {
        source: WorktreeSourceInfo,
        worktrees: Vec<WorktreeInfo>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorktreeCreateResult {
    WorktreeCreated {
        workspace: WorkspaceInfo,
        root_pane: PaneInfo,
        worktree: WorktreeInfo,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorktreeOpenResult {
    WorktreeOpened {
        workspace: WorkspaceInfo,
        root_pane: PaneInfo,
        worktree: WorktreeInfo,
        already_open: bool,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PaneSplitResult {
    PaneInfo { pane: PaneInfo },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PaneRunResult {
    Ok {},
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorktreeRemoveResult {
    WorktreeRemoved {
        workspace_id: String,
        path: String,
        forced: bool,
    },
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
                .map(|worktree| worktree.checkout_path.as_str()),
            Some("/worktrees/repo/feature")
        );
    }

    #[test]
    fn unknown_agent_status_maps_to_unknown() {
        let future_workspace = WORKSPACE.replace(
            r#""agent_status":"unknown""#,
            r#""agent_status":"some_future_status""#,
        );
        let json = format!(
            r#"{{"id":"cli:workspace:list","result":{{"type":"workspace_list","workspaces":[{future_workspace}]}}}}"#
        );
        let WorkspaceListResult::WorkspaceList { workspaces } = parse_success(&json);
        assert_eq!(workspaces[0].agent_status, AgentStatus::Unknown);
    }

    #[test]
    fn parses_realistic_worktree_list_with_open_and_closed_entries() {
        let json = format!(
            r#"{{"id":"cli:worktree:list","result":{{"type":"worktree_list","source":{{"repo_key":"/repo/.git","repo_name":"repo","repo_root":"/repo","source_checkout_path":"/repo"}},"worktrees":[{WORKTREE_CLOSED},{WORKTREE_OPEN}]}}}}"#
        );
        let WorktreeListResult::WorktreeList { source, worktrees } = parse_success(&json);
        assert_eq!(source.repo_root, "/repo");
        assert!(worktrees[0].open_workspace_id.is_none());
        assert_eq!(worktrees[1].open_workspace_id.as_deref(), Some("w_2"));
    }

    #[test]
    fn parses_create_open_and_remove_response_shapes() {
        let created = format!(
            r#"{{"id":"create","result":{{"type":"worktree_created","workspace":{WORKSPACE_WITH_WORKTREE},"tab":{{}},"root_pane":{ROOT_PANE},"worktree":{WORKTREE_OPEN}}}}}"#
        );
        let WorktreeCreateResult::WorktreeCreated {
            workspace,
            worktree,
            ..
        } = parse_success(&created);
        assert_eq!(workspace.workspace_id, "w_2");
        assert_eq!(worktree.branch.as_deref(), Some("feature"));

        let opened = format!(
            r#"{{"id":"open","result":{{"type":"worktree_opened","workspace":{WORKSPACE_WITH_WORKTREE},"tab":{{}},"root_pane":{ROOT_PANE},"worktree":{WORKTREE_OPEN},"already_open":true}}}}"#
        );
        let WorktreeOpenResult::WorktreeOpened { already_open, .. } = parse_success(&opened);
        assert!(already_open);

        let removed = r#"{"id":"remove","result":{"type":"worktree_removed","workspace_id":"w_2","path":"/worktrees/repo/feature","forced":true}}"#;
        let WorktreeRemoveResult::WorktreeRemoved { forced, .. } = parse_success(removed);
        assert!(forced);
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
        let binary = temp.path().join("fake-herdr");
        let args_file = temp.path().join("args");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\nprintf '%s\\n' '{}'\n",
            args_file.display(),
            response
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
    fn cli_builds_exact_pane_split_and_run_calls() {
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

        let run_temp = TempDir::new().unwrap();
        let (binary, args_file) = fake_herdr(&run_temp, r#"{"id":"run","result":{"type":"ok"}}"#);
        let provider = CliHerdrProvider::new(binary);
        retry_fake_herdr(|| provider.pane_run("p_2", "printf ONOPEN_OK"));
        assert_eq!(
            fs::read_to_string(args_file).unwrap().trim(),
            "pane run p_2 printf ONOPEN_OK"
        );
    }

    #[test]
    fn cli_rejects_relative_paths_before_invocation() {
        let provider = CliHerdrProvider::new("/does/not/exist");
        assert!(matches!(
            provider.worktree_list(Path::new("relative")),
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
