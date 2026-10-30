use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool, mpsc},
    time::Duration,
};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;
use crate::{
    config::{OnOpenConfig, OnOpenPaneConfig, OnOpenPaneDirection},
    event::AppEvent,
    git::{
        CliGitProvider,
        test_fixtures::{repo_fixture, run_test_git},
    },
    spawn::{EventSender, spawn_open_repo},
};

#[derive(Debug, Clone)]
struct FakePane {
    pane: String,
    terminal: String,
    start: PathBuf,
    live: PathBuf,
}

#[derive(Debug, Clone)]
struct FakeSpace {
    id: String,
    machine: Option<String>,
    panes: Vec<FakePane>,
}

#[derive(Default)]
struct FakeState {
    spaces: Vec<FakeSpace>,
    next_id: usize,
    commands: Vec<(String, Value)>,
    sent: Vec<(String, String)>,
    focused_pane: Option<String>,
    /// A folder every split starts in, instead of the split pane's.
    split_folder: Option<PathBuf>,
}

/// The parts of `herdr-app` and `hc` the app backend uses, over an in-memory window.
#[derive(Default)]
struct FakeApp {
    state: Mutex<FakeState>,
}

impl FakeApp {
    fn add_space(&self, folder: &Path) -> FakeSpace {
        let mut state = self.state.lock().unwrap();
        let space = FakeSpace {
            id: format!("sp{}", state.next_id),
            machine: None,
            panes: vec![new_pane(&mut state, folder)],
        };
        state.next_id += 1;
        state.spaces.push(space.clone());
        space
    }

    fn add_remote_space(&self, folder: &Path) {
        let mut state = self.state.lock().unwrap();
        let mut pane = new_pane(&mut state, folder);
        pane.terminal = format!("vps/{}", pane.terminal);
        state.spaces.push(FakeSpace {
            id: "vps/sp-remote".into(),
            machine: Some("vps".into()),
            panes: vec![pane],
        });
    }

    fn command_names(&self) -> Vec<String> {
        let state = self.state.lock().unwrap();
        state
            .commands
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn commands(&self) -> Vec<(String, Value)> {
        self.state.lock().unwrap().commands.clone()
    }

    fn spaces(&self) -> Vec<FakeSpace> {
        self.state.lock().unwrap().spaces.clone()
    }
}

fn new_pane(state: &mut FakeState, folder: &Path) -> FakePane {
    let id = state.next_id;
    state.next_id += 1;
    FakePane {
        pane: format!("p{id}"),
        terminal: format!("t{id}"),
        start: folder.to_path_buf(),
        live: folder.to_path_buf(),
    }
}

fn refused(message: &str) -> HerdrError {
    HerdrError::Other {
        code: "herdr_app".into(),
        message: message.into(),
    }
}

impl AppHost for FakeApp {
    fn herdr_app(&self, args: &[String]) -> Result<Vec<u8>, HerdrError> {
        let mut state = self.state.lock().unwrap();
        if args == ["ls"] {
            let spaces: Vec<Value> = state
                .spaces
                .iter()
                .map(|space| {
                    json!({
                        "id": space.id,
                        "machine": space.machine,
                        "tabs": [{ "id": "tb", "terminals": space.panes.iter().map(|pane| json!({
                            "terminal": pane.terminal, "pane": pane.pane, "sub_agents": []
                        })).collect::<Vec<_>>() }],
                    })
                })
                .collect();
            return Ok(json!({ "spaces": spaces, "recent": [] }).to_string().into());
        }
        let [verb, name, args] = args else {
            panic!("unexpected herdr-app call {args:?}");
        };
        assert_eq!(verb, "do");
        let args: Value = serde_json::from_str(args).unwrap();
        state.commands.push((name.clone(), args.clone()));
        let output = match name.as_str() {
            "space.new" => {
                let folder = PathBuf::from(args["folder"].as_str().unwrap());
                let pane = new_pane(&mut state, &folder);
                let id = format!("sp{}", state.next_id);
                state.next_id += 1;
                let output = json!({ "space": id, "tab": "tb", "terminal": pane.terminal });
                state.spaces.push(FakeSpace {
                    id,
                    machine: None,
                    panes: vec![pane],
                });
                output
            }
            "space.select" => {
                let id = args["id"].as_str().unwrap();
                if !state.spaces.iter().any(|space| space.id == id) {
                    return Err(refused("no such space"));
                }
                json!({ "ok": true })
            }
            "space.close" => {
                assert_eq!(args["confirmed"], true);
                let id = args["id"].as_str().unwrap();
                state.spaces.retain(|space| space.id != id);
                json!({ "ok": true })
            }
            "pane.split" => {
                let target = args["pane"].as_str().unwrap().to_string();
                let index = state
                    .spaces
                    .iter()
                    .position(|space| space.panes.iter().any(|pane| pane.pane == target))
                    .ok_or_else(|| refused("no such pane"))?;
                let folder = state.split_folder.clone().unwrap_or_else(|| {
                    state.spaces[index]
                        .panes
                        .iter()
                        .find(|pane| pane.pane == target)
                        .unwrap()
                        .live
                        .clone()
                });
                let pane = new_pane(&mut state, &folder);
                state.focused_pane = Some(pane.pane.clone());
                let output = json!({ "pane": pane.pane, "terminal": pane.terminal });
                state.spaces[index].panes.push(pane);
                output
            }
            "pane.focus" => {
                state.focused_pane = Some(args["pane"].as_str().unwrap().into());
                json!({ "ok": true })
            }
            "pane.close" => {
                let target = args["pane"].as_str().unwrap();
                for space in &mut state.spaces {
                    space.panes.retain(|pane| pane.pane != target);
                }
                json!({ "ok": true })
            }
            other => panic!("unexpected command {other}"),
        };
        Ok(output.to_string().into())
    }

    fn hc(&self, args: &[String]) -> Result<Vec<u8>, HerdrError> {
        let mut state = self.state.lock().unwrap();
        let pane = |state: &FakeState, terminal: &str| {
            state
                .spaces
                .iter()
                .flat_map(|space| &space.panes)
                .find(|pane| pane.terminal == terminal)
                .cloned()
        };
        match args {
            [get, terminal, flag] if get == "get" && flag == "--json" => {
                assert!(
                    !terminal.contains('/'),
                    "asked hc for another machine's terminal {terminal}"
                );
                let pane = pane(&state, terminal).expect("hc get of an unknown terminal");
                Ok(json!({
                    "address": terminal,
                    "cwd": pane.start,
                    "foreground": { "cwd": pane.live, "name": "zsh" },
                })
                .to_string()
                .into())
            }
            [send, terminal, text] if send == "send" => {
                pane(&state, terminal).expect("hc send to an unknown terminal");
                state.sent.push((terminal.clone(), text.clone()));
                Ok(br#"{"started":null,"status":null,"turn":0}"#.to_vec())
            }
            _ => panic!("unexpected hc call {args:?}"),
        }
    }
}

struct Fixture {
    temp: TempDir,
    repo: PathBuf,
    app: Arc<FakeApp>,
    provider: AppHerdrProvider,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let repo = repo_fixture(temp.path(), "repo");
        let app = Arc::new(FakeApp::default());
        let provider = AppHerdrProvider::new(
            Arc::clone(&app) as Arc<dyn AppHost>,
            Arc::new(CliGitProvider),
            temp.path().join("worktrees"),
            temp.path().join("notices").join("kiosk.jsonl"),
        );
        Self {
            temp,
            repo,
            app,
            provider,
        }
    }

    fn linked(&self, branch: &str) -> PathBuf {
        let path = self.temp.path().join(format!("repo-{branch}"));
        run_test_git(
            &self.repo,
            &["worktree", "add", "-b", branch, path.to_str().unwrap()],
        );
        path
    }

    fn worktree_root(&self) -> PathBuf {
        self.temp.path().join("worktrees")
    }
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

fn same_path(left: &str, right: &Path) -> bool {
    canonical(Path::new(left)) == canonical(right)
}

#[test]
fn opening_a_closed_repo_makes_an_unnamed_space_in_it() {
    let fixture = Fixture::new();

    let response = fixture
        .provider
        .worktree_open(
            &fixture.repo,
            &WorktreeOpenTarget::Path(fixture.repo.clone()),
            true,
        )
        .unwrap();

    assert_eq!(response.already_open, Some(false));
    let spaces = fixture.app.spaces();
    assert_eq!(spaces.len(), 1);
    let opened = response.opened.unwrap();
    assert_eq!(opened.root_pane_id, spaces[0].panes[0].pane);
    assert!(same_path(&opened.path, &fixture.repo));
    let commands = fixture.app.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].0, "space.new");
    assert_eq!(
        commands[0]
            .1
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["folder"],
        "the space must stay unnamed so the app names it after its folder"
    );
}

#[test]
fn opening_a_repo_already_shown_selects_its_space() {
    let fixture = Fixture::new();
    let subdir = fixture.repo.join("src");
    fs::create_dir(&subdir).unwrap();
    fixture.app.add_space(Path::new("/elsewhere"));
    let shown = fixture.app.add_space(&subdir);

    let response = fixture
        .provider
        .worktree_open(
            &fixture.repo,
            &WorktreeOpenTarget::Path(fixture.repo.clone()),
            true,
        )
        .unwrap();

    assert_eq!(response.already_open, Some(true));
    assert_eq!(response.opened.unwrap().root_pane_id, shown.panes[0].pane);
    assert_eq!(
        fixture.app.commands(),
        [("space.select".into(), json!({ "id": shown.id }))]
    );
}

#[test]
fn a_space_follows_its_first_terminals_live_folder() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(Path::new("/elsewhere"));
    fixture.app.state.lock().unwrap().spaces[0].panes[0].live = fixture.repo.clone();

    let listed = fixture.provider.worktree_list(&fixture.repo).unwrap();

    assert_eq!(
        listed.worktrees[0].open_workspace_id.as_deref(),
        Some(space.id.as_str())
    );
}

#[test]
fn opening_a_branch_uses_its_checkout() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");

    let response = fixture
        .provider
        .worktree_open(
            &fixture.repo,
            &WorktreeOpenTarget::Branch("feature".into()),
            true,
        )
        .unwrap();

    assert_eq!(response.already_open, Some(false));
    assert!(same_path(&response.opened.unwrap().path, &feature));
    let error = fixture
        .provider
        .worktree_open(
            &fixture.repo,
            &WorktreeOpenTarget::Branch("missing".into()),
            true,
        )
        .unwrap_err();
    assert!(matches!(error, HerdrError::WorktreeNotFound(_)), "{error}");
}

#[test]
fn a_checkout_nested_in_the_repo_owns_the_spaces_inside_it() {
    let fixture = Fixture::new();
    let nested = fixture.repo.join(".worktrees").join("nested");
    run_test_git(
        &fixture.repo,
        &["worktree", "add", "-b", "nested", nested.to_str().unwrap()],
    );
    let space = fixture.app.add_space(&nested);

    let listed = fixture.provider.worktree_list(&fixture.repo).unwrap();

    let [main, nested] = listed.worktrees.as_slice() else {
        panic!("expected two checkouts, got {:?}", listed.worktrees);
    };
    assert!(same_path(&main.path, &fixture.repo));
    assert_eq!(main.open_workspace_id, None);
    assert_eq!(nested.branch.as_deref(), Some("nested"));
    assert_eq!(nested.open_workspace_id, Some(space.id));
}

#[test]
fn creating_a_branch_checks_it_out_beside_the_herdr_checkouts() {
    let fixture = Fixture::new();

    let response = fixture
        .provider
        .worktree_create(&WorktreeCreateRequest {
            cwd: fixture.repo.clone(),
            branch: "Feature/Login_Page.v2".into(),
            base: Some("HEAD".into()),
            path: None,
            focus: true,
        })
        .unwrap();

    let expected = fixture
        .worktree_root()
        .join("repo")
        .join("feature-login-page-v2");
    let opened = response.opened.unwrap();
    assert!(same_path(&opened.path, &expected));
    let listed = fixture.provider.worktree_list(&fixture.repo).unwrap();
    let created = listed
        .worktrees
        .iter()
        .find(|worktree| worktree.branch.as_deref() == Some("Feature/Login_Page.v2"))
        .unwrap();
    assert!(same_path(&created.path, &expected));
    assert!(created.open_workspace_id.is_some());
}

#[test]
fn creating_an_existing_branch_without_a_base_checks_it_out() {
    let fixture = Fixture::new();
    run_test_git(&fixture.repo, &["branch", "existing"]);
    let path = fixture.temp.path().join("custom");

    fixture
        .provider
        .worktree_create(&WorktreeCreateRequest {
            cwd: fixture.repo.clone(),
            branch: "existing".into(),
            base: None,
            path: Some(path.clone()),
            focus: true,
        })
        .unwrap();

    assert!(path.join("README.md").exists());
    let error = fixture
        .provider
        .worktree_create(&WorktreeCreateRequest {
            cwd: fixture.repo.clone(),
            branch: "existing".into(),
            base: None,
            path: Some(fixture.temp.path().join("again")),
            focus: true,
        })
        .unwrap_err();
    assert!(
        matches!(error, HerdrError::WorktreeCreateFailed(_)),
        "{error}"
    );
    assert_eq!(fixture.app.spaces().len(), 1);
}

#[test]
fn removing_a_worktree_deletes_the_checkout_then_closes_its_space() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    let space = fixture.app.add_space(&feature);

    let response = fixture.provider.worktree_remove(&space.id, false).unwrap();

    assert!(response.warning.is_none());
    assert!(!feature.exists());
    assert!(fixture.app.spaces().is_empty());
    assert_eq!(fixture.app.command_names(), ["space.close"]);
}

#[test]
fn removing_a_dirty_worktree_needs_force_and_keeps_the_space() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    fs::write(feature.join("scratch.txt"), "unsaved").unwrap();
    let space = fixture.app.add_space(&feature);

    let error = fixture
        .provider
        .worktree_remove(&space.id, false)
        .unwrap_err();

    assert!(
        matches!(error, HerdrError::DirtyWorktreeRequiresForce(_)),
        "{error}"
    );
    assert!(feature.exists());
    assert_eq!(fixture.app.spaces().len(), 1);

    fixture.provider.worktree_remove(&space.id, true).unwrap();
    assert!(!feature.exists());
    assert!(fixture.app.spaces().is_empty());
}

#[test]
fn the_main_checkout_is_never_removed() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(&fixture.repo);

    let error = fixture
        .provider
        .worktree_remove(&space.id, true)
        .unwrap_err();

    assert!(matches!(error, HerdrError::NotGitWorktree(_)), "{error}");
    assert!(fixture.repo.exists());
    assert_eq!(fixture.app.commands(), []);
}

#[test]
fn spaces_report_their_repository_and_other_machines_are_ignored() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    let plain = fixture.temp.path().join("plain");
    fs::create_dir(&plain).unwrap();
    fixture.app.add_space(&fixture.repo);
    fixture.app.add_space(&feature);
    fixture.app.add_space(&plain);
    fixture.app.add_remote_space(&fixture.repo);

    let workspaces = fixture.provider.workspace_list().unwrap();

    let roots: Vec<_> = workspaces
        .iter()
        .map(|workspace| {
            workspace
                .worktree
                .as_ref()
                .map(|worktree| canonical(Path::new(&worktree.repo_root)))
        })
        .collect();
    let repo = Some(canonical(&fixture.repo));
    assert_eq!(roots, [repo.clone(), repo, None]);

    let panes = fixture.provider.pane_list().unwrap();
    assert_eq!(panes.len(), 3);
    assert!(same_path(panes[2].cwd.as_deref().unwrap(), &plain));
}

#[test]
fn folders_open_as_spaces_and_focus_selects_them() {
    let fixture = Fixture::new();
    let plain = fixture.temp.path().join("plain");
    fs::create_dir(&plain).unwrap();

    let created = fixture.provider.workspace_create(&plain, true).unwrap();
    let id = created.workspace_id.unwrap();
    fixture.provider.workspace_focus(&id).unwrap();

    assert_eq!(fixture.app.command_names(), ["space.new", "space.select"]);
    assert!(fixture.provider.workspace_create(&plain, false).is_err());
    assert!(fixture.provider.workspace_focus("missing").is_err());
}

fn split_request(pane_id: &str, cwd: &Path) -> PaneSplitRequest {
    PaneSplitRequest {
        pane_id: pane_id.into(),
        direction: OnOpenPaneDirection::Right,
        ratio: None,
        cwd: cwd.to_path_buf(),
        focus: false,
    }
}

#[test]
fn splitting_without_focus_gives_focus_back_to_the_split_pane() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(&fixture.repo);
    let root = &space.panes[0].pane;

    let split = fixture
        .provider
        .pane_split(&split_request(root, &fixture.repo))
        .unwrap();

    assert_ne!(&split.pane_id, root);
    assert_eq!(
        fixture.app.commands(),
        [
            (
                "pane.split".into(),
                json!({ "direction": "right", "pane": root })
            ),
            ("pane.focus".into(), json!({ "pane": root })),
        ]
    );
    assert_eq!(
        fixture.app.state.lock().unwrap().focused_pane.as_ref(),
        Some(root)
    );
}

#[test]
fn a_split_ratio_is_refused_because_the_app_cannot_size_splits() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(&fixture.repo);
    let mut request = split_request(&space.panes[0].pane, &fixture.repo);
    request.ratio = Some(0.6);

    let error = fixture.provider.pane_split(&request).unwrap_err();

    assert!(error.to_string().contains("ratio"), "{error}");
    assert_eq!(fixture.app.commands(), []);
}

#[test]
fn a_split_that_starts_elsewhere_is_closed_and_reported() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(&fixture.repo);
    fixture.app.state.lock().unwrap().split_folder = Some("/elsewhere".into());

    let error = fixture
        .provider
        .pane_split(&split_request(&space.panes[0].pane, &fixture.repo))
        .unwrap_err();

    assert!(error.to_string().contains("/elsewhere"), "{error}");
    assert_eq!(fixture.app.spaces()[0].panes.len(), 1);
}

#[test]
fn running_a_command_types_it_into_the_panes_shell() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(&fixture.repo);

    fixture
        .provider
        .pane_run(&space.panes[0].pane, "hx .")
        .unwrap();

    assert_eq!(
        fixture.app.state.lock().unwrap().sent,
        [(space.panes[0].terminal.clone(), "hx .".into())]
    );
    assert!(fixture.provider.pane_run("missing", "hx").is_err());
}

#[test]
fn notifications_are_appended_as_json_lines() {
    let fixture = Fixture::new();

    fixture
        .provider
        .notification_show("herdr-kiosk", "first")
        .unwrap();
    fixture
        .provider
        .notification_show("herdr-kiosk", "second \"quoted\"")
        .unwrap();

    let contents = fs::read_to_string(fixture.temp.path().join("notices/kiosk.jsonl")).unwrap();
    let bodies: Vec<String> = contents
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["body"].to_string())
        .collect();
    assert_eq!(bodies, [r#""first""#, r#""second \"quoted\"""#]);
}

#[test]
fn opening_a_repo_runs_on_open_panes_in_the_new_space() {
    let fixture = Fixture::new();
    let provider: Arc<dyn HerdrProvider> = Arc::new(fixture.provider);
    let (tx, rx) = mpsc::channel();
    let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
    let on_open = OnOpenConfig {
        panes: vec![OnOpenPaneConfig {
            command: "hx".into(),
            direction: OnOpenPaneDirection::Right,
            ratio: None,
        }],
    };

    spawn_open_repo(&provider, &sender, fixture.repo.clone(), on_open);

    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        AppEvent::RepoOpened { warning: None }
    ));
    let spaces = fixture.app.spaces();
    assert_eq!(spaces.len(), 1);
    let [root, editor] = spaces[0].panes.as_slice() else {
        panic!("expected two panes, got {:?}", spaces[0].panes);
    };
    assert_eq!(
        fixture.app.state.lock().unwrap().sent,
        [(editor.terminal.clone(), "hx".into())]
    );
    assert_eq!(
        fixture.app.state.lock().unwrap().focused_pane.as_ref(),
        Some(&root.pane)
    );
}

#[test]
fn branch_slugs_match_the_herdr_tui() {
    assert_eq!(branch_slug("worktree/brave-river"), "worktree-brave-river");
    assert_eq!(
        branch_slug("issue/137 Worktree Spaces"),
        "issue-137-worktree-spaces"
    );
    assert_eq!(branch_slug("///"), "worktree");
}

#[cfg(unix)]
mod cli {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn script(temp: &TempDir, name: &str, body: &str) -> PathBuf {
        let path = temp.path().join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn refusals_from_either_program_keep_their_message() {
        let temp = TempDir::new().unwrap();
        let app = script(
            &temp,
            "herdr-app",
            r#"printf '{"error":"no such space"}'; exit 1"#,
        );
        let hc = script(
            &temp,
            "hc",
            r#"printf '{"error":{"kind":"not_found","message":"no terminal t9"}}' >&2; exit 3"#,
        );
        let host = CliAppHost::new(app, hc);

        assert_eq!(
            host.herdr_app(&["do".into()]).unwrap_err(),
            refused("no such space")
        );
        assert_eq!(
            host.hc(&["get".into()]).unwrap_err(),
            HerdrError::Other {
                code: "not_found".into(),
                message: "no terminal t9".into(),
            }
        );
    }

    #[test]
    fn other_failures_report_the_exit_and_stderr() {
        let temp = TempDir::new().unwrap();
        let app = script(&temp, "herdr-app", "echo 'window is gone' >&2; exit 2");
        let host = CliAppHost::new(app, temp.path().join("missing-hc"));

        let error = host.herdr_app(&["ls".into()]).unwrap_err();
        assert!(
            matches!(&error, HerdrError::Invocation(message) if message.contains("window is gone")),
            "{error}"
        );
        assert!(matches!(
            host.hc(&["ls".into()]).unwrap_err(),
            HerdrError::Invocation(_)
        ));
    }
}
