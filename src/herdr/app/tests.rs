use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, atomic::AtomicBool, mpsc},
    time::Duration,
};

use kiosk_core::{
    backend::KnownCheckout,
    event::{AppEvent, WorktreeRemovalOutcome},
    git::{
        CliGitProvider,
        test_fixtures::{repo_fixture, run_test_git},
    },
    spawn::{EventSender, spawn_create_new_branch, spawn_open_branch, spawn_worktree_removal},
};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;
use kiosk_core::config::{
    OnOpenConfig,
    on_open::{OnOpenPaneConfig, OnOpenPaneDirection},
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
    /// What each new terminal had typed into it as it started (`type`).
    typed: Vec<(String, String)>,
    focused_pane: Option<String>,
}

/// The parts of `herdr-app` and `hc` the app backend uses, over an in-memory window.
#[derive(Default)]
struct FakeApp {
    state: Mutex<FakeState>,
    /// When set, each `hc get` waits (up to a deadline) for this many to be in flight at once.
    rendezvous: Option<usize>,
    /// `hc get`s in flight now, and the most there have been at once.
    gets: (Mutex<(usize, usize)>, Condvar),
}

impl FakeApp {
    fn meet_other_gets(&self, want: usize) {
        let (gets, changed) = &self.gets;
        let mut gets = gets.lock().unwrap();
        gets.0 += 1;
        gets.1 = gets.1.max(gets.0);
        changed.notify_all();
        let (mut gets, _) = changed
            .wait_timeout_while(gets, Duration::from_secs(2), |gets| gets.1 < want)
            .unwrap();
        gets.0 -= 1;
    }
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
                let folder = args["folder"].as_str().map_or_else(
                    || {
                        state.spaces[index]
                            .panes
                            .iter()
                            .find(|pane| pane.pane == target)
                            .unwrap()
                            .live
                            .clone()
                    },
                    PathBuf::from,
                );
                let pane = new_pane(&mut state, &folder);
                if let Some(line) = args["type"].as_str() {
                    state.typed.push((pane.terminal.clone(), line.into()));
                }
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
        if let Some(want) = self.rendezvous {
            self.meet_other_gets(want);
        }
        let state = self.state.lock().unwrap();
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
            _ => panic!("unexpected hc call {args:?}"),
        }
    }
}

struct Fixture {
    temp: TempDir,
    repo: PathBuf,
    app: Arc<FakeApp>,
    backend: Arc<HerdrApp>,
}

fn backend(app: &Arc<FakeApp>, temp: &Path, on_open: OnOpenConfig) -> HerdrApp {
    HerdrApp::new(
        Arc::clone(app) as Arc<dyn AppHost>,
        Arc::new(CliGitProvider),
        WorktreePlacement::new(temp.join("worktrees")).unwrap(),
        temp.join("notices").join("kiosk.jsonl"),
        on_open,
    )
}

impl Fixture {
    fn new() -> Self {
        Self::with_on_open(OnOpenConfig::default())
    }

    fn with_on_open(on_open: OnOpenConfig) -> Self {
        let temp = TempDir::new().unwrap();
        let repo = repo_fixture(temp.path(), "repo");
        let app = Arc::new(FakeApp::default());
        let backend = Arc::new(backend(&app, temp.path(), on_open));
        Self {
            temp,
            repo,
            app,
            backend,
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

    fn checkout_state(&self, path: &Path) -> CheckoutState<SpaceId> {
        self.backend
            .checkouts(&self.repo)
            .unwrap()
            .into_iter()
            .find(|state| canonical(&state.path) == canonical(path))
            .unwrap_or_else(|| panic!("{} is not a checkout", path.display()))
    }

    /// Runs one of kiosk's branch operations against this backend and returns its event.
    fn run(
        &self,
        start: impl FnOnce(&Arc<dyn GitProvider>, &Arc<HerdrApp>, &EventSender),
    ) -> AppEvent {
        next(&self.start(start))
    }

    /// Starts one of kiosk's branch operations and returns the events it sends.
    fn start(
        &self,
        start: impl FnOnce(&Arc<dyn GitProvider>, &Arc<HerdrApp>, &EventSender),
    ) -> mpsc::Receiver<AppEvent> {
        let git: Arc<dyn GitProvider> = Arc::new(CliGitProvider);
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
        start(&git, &self.backend, &sender);
        rx
    }

    /// Removes `checkout` as the picker does, which lets the space close once git has
    /// removed the checkout.
    fn remove(&self, branch: &str, checkout: &Path, force: bool) -> WorktreeRemovalOutcome {
        let events = self.start(|git, backend, sender| {
            spawn_worktree_removal(
                git,
                backend,
                sender,
                crate::PRODUCT.backend_name,
                self.repo.clone(),
                branch.into(),
                checkout.into(),
                force,
            );
        });
        let mut event = next(&events);
        if let AppEvent::CheckoutRemoved { gate, .. } = event {
            // Dropping the gate lets the removal go on to close the space.
            drop(gate);
            event = next(&events);
        }
        let AppEvent::WorktreeRemovalFinished { outcome, .. } = event else {
            panic!("expected a removal, got {event:?}");
        };
        outcome
    }
}

fn next(events: &mpsc::Receiver<AppEvent>) -> AppEvent {
    events.recv_timeout(Duration::from_secs(10)).unwrap()
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

#[test]
fn opening_a_closed_repo_makes_an_unnamed_space_in_it() {
    let fixture = Fixture::new();

    let opened = fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();

    assert_eq!(opened, Opened::default());
    assert_eq!(fixture.app.spaces().len(), 1);
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
    let fixture = Fixture::with_on_open(editor_on_open());
    let subdir = fixture.repo.join("src");
    fs::create_dir(&subdir).unwrap();
    fixture.app.add_space(Path::new("/elsewhere"));
    let shown = fixture.app.add_space(&subdir);

    fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();

    assert_eq!(
        fixture.app.commands(),
        [("space.select".into(), json!({ "id": shown.id }))],
        "an existing space gets no on_open panes"
    );
}

#[test]
fn a_space_follows_its_first_terminals_live_folder() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(Path::new("/elsewhere"));
    fixture.app.state.lock().unwrap().spaces[0].panes[0].live = fixture.repo.clone();

    assert_eq!(
        fixture.checkout_state(&fixture.repo).session,
        Some(SpaceId(space.id))
    );
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

    let checkouts = fixture.backend.checkouts(&fixture.repo).unwrap();

    let [main, nested_state] = checkouts.as_slice() else {
        panic!("expected two checkouts, got {checkouts:?}");
    };
    assert_eq!(canonical(&main.path), canonical(&fixture.repo));
    assert_eq!(main.session, None);
    assert_eq!(canonical(&nested_state.path), canonical(&nested));
    assert_eq!(nested_state.session, Some(SpaceId(space.id)));
}

#[test]
fn opening_a_branch_uses_its_checkout() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");

    let event = fixture.run(|git, backend, sender| {
        spawn_open_branch(
            git,
            backend,
            sender,
            fixture.repo.clone(),
            "feature".into(),
            KnownCheckout::Exists,
        );
    });

    assert!(
        matches!(event, AppEvent::RepoOpened { warning: None }),
        "{event:?}"
    );
    let spaces = fixture.app.spaces();
    assert_eq!(spaces.len(), 1);
    assert_eq!(canonical(&spaces[0].panes[0].start), canonical(&feature));
}

#[test]
fn creating_a_branch_checks_it_out_beside_the_herdr_checkouts() {
    let fixture = Fixture::new();

    let event = fixture.run(|git, backend, sender| {
        spawn_create_new_branch(
            git,
            backend,
            sender,
            fixture.repo.clone(),
            "Feature/Login_Page.v2".into(),
            "HEAD".into(),
        );
    });

    assert!(
        matches!(event, AppEvent::RepoOpened { warning: None }),
        "{event:?}"
    );
    let expected = fixture
        .worktree_root()
        .join("repo")
        .join("feature-login-page-v2");
    assert!(expected.join("README.md").exists());
    assert!(fixture.checkout_state(&expected).session.is_some());
}

#[test]
fn a_branch_without_a_checkout_gets_one_in_the_herdr_worktree_folder() {
    let fixture = Fixture::new();
    run_test_git(&fixture.repo, &["branch", "existing"]);

    let event = fixture.run(|git, backend, sender| {
        spawn_open_branch(
            git,
            backend,
            sender,
            fixture.repo.clone(),
            "existing".into(),
            KnownCheckout::Missing,
        );
    });

    assert!(
        matches!(event, AppEvent::RepoOpened { warning: None }),
        "{event:?}"
    );
    let checkout = fixture.worktree_root().join("repo").join("existing");
    assert!(checkout.join("README.md").exists());
    assert_eq!(fixture.app.spaces().len(), 1);
}

#[test]
fn removing_a_worktree_deletes_the_checkout_then_closes_its_space() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    fixture.app.add_space(&feature);

    let outcome = fixture.remove("feature", &feature, false);

    assert_eq!(outcome, WorktreeRemovalOutcome::Removed { warning: None });
    assert!(!feature.exists());
    assert!(fixture.app.spaces().is_empty());
    assert_eq!(fixture.app.command_names(), ["space.close"]);
}

#[test]
fn removing_a_dirty_worktree_needs_force_and_keeps_the_space() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    fs::write(feature.join("scratch.txt"), "unsaved").unwrap();
    fixture.app.add_space(&feature);

    assert_eq!(
        fixture.remove("feature", &feature, false),
        WorktreeRemovalOutcome::DirtyRequiresForce
    );
    assert!(feature.exists());
    assert_eq!(fixture.app.spaces().len(), 1);

    assert_eq!(
        fixture.remove("feature", &feature, true),
        WorktreeRemovalOutcome::Removed { warning: None }
    );
    assert!(!feature.exists());
    assert!(fixture.app.spaces().is_empty());
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

    let roots: Vec<_> = fixture
        .backend
        .open_repos()
        .unwrap()
        .iter()
        .map(|root| canonical(root))
        .collect();
    assert_eq!(roots, [canonical(&fixture.repo), canonical(&fixture.repo)]);

    let folders = fixture.backend.open_folders().unwrap();
    assert_eq!(folders.len(), 3);
    assert_eq!(canonical(&folders[2]), canonical(&plain));
}

#[test]
fn folders_open_as_spaces_and_an_open_folder_selects_its_space() {
    let fixture = Fixture::with_on_open(editor_on_open());
    let plain = fixture.temp.path().join("plain");
    fs::create_dir(&plain).unwrap();

    fixture.backend.open_folder(&plain).unwrap();
    fixture.backend.open_folder(&plain).unwrap();

    assert_eq!(
        fixture.app.command_names(),
        ["space.new", "space.select"],
        "folders get no on_open panes"
    );
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
fn a_pane_opens_with_its_command_typed_in_one_split_and_gives_focus_back() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(&fixture.repo);
    let root = &space.panes[0].pane;

    let split = fixture
        .backend
        .pane_open(&split_request(root, &fixture.repo), "hx .")
        .unwrap();

    assert_ne!(&split.pane_id, root);
    let folder = fixture.repo.to_str().unwrap();
    assert_eq!(
        fixture.app.commands(),
        [
            (
                "pane.split".into(),
                json!({ "direction": "right", "pane": root, "folder": folder, "type": "hx ." })
            ),
            ("pane.focus".into(), json!({ "pane": root })),
        ]
    );
    let state = fixture.app.state.lock().unwrap();
    let new = &state.spaces[0].panes[1];
    assert_eq!(
        (new.pane.as_str(), new.start.as_path()),
        (split.pane_id.as_str(), fixture.repo.as_path())
    );
    assert_eq!(state.typed, [(new.terminal.clone(), "hx .".into())]);
    assert_eq!(state.focused_pane.as_ref(), Some(root));
}

#[test]
fn a_pane_starts_in_the_checkout_wherever_the_split_pane_has_gone() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(&fixture.repo);
    fixture.app.state.lock().unwrap().spaces[0].panes[0].live = "/elsewhere".into();

    fixture
        .backend
        .pane_open(&split_request(&space.panes[0].pane, &fixture.repo), "hx")
        .unwrap();

    assert_eq!(fixture.app.spaces()[0].panes[1].start, fixture.repo);
}

#[test]
fn a_pane_that_cannot_open_as_asked_is_refused_before_the_app_is_called() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(&fixture.repo);
    let mut ratio = split_request(&space.panes[0].pane, &fixture.repo);
    ratio.ratio = Some(0.6);
    let relative = split_request(&space.panes[0].pane, Path::new("repo"));
    let request = split_request(&space.panes[0].pane, &fixture.repo);

    for (request, command, why) in [
        (&ratio, "hx", "ratio"),
        (&relative, "hx", "absolute"),
        (&request, "", "command"),
    ] {
        let Err(PaneOpenError::Split(error)) = fixture.backend.pane_open(request, command) else {
            panic!("{why}: expected a refused split");
        };
        assert!(error.to_string().contains(why), "{error}");
    }
    assert_eq!(fixture.app.commands(), []);
}

#[test]
fn a_refused_split_is_reported_as_no_pane_made() {
    let fixture = Fixture::new();

    let error = fixture
        .backend
        .pane_open(&split_request("missing", &fixture.repo), "hx")
        .unwrap_err();

    assert_eq!(error, PaneOpenError::Split(refused("no such pane")));
}

#[test]
fn every_terminal_folder_is_looked_up_at_once_and_kept_with_its_pane() {
    let temp = TempDir::new().unwrap();
    let folders: Vec<PathBuf> = (0..5).map(|i| temp.path().join(format!("f{i}"))).collect();
    let app = Arc::new(FakeApp {
        rendezvous: Some(folders.len()),
        ..FakeApp::default()
    });
    for folder in &folders {
        app.add_space(folder);
    }
    let backend = backend(&app, temp.path(), OnOpenConfig::default());

    let spaces = backend.spaces().unwrap();

    assert_eq!(
        app.gets.0.lock().unwrap().1,
        folders.len(),
        "all in flight together"
    );
    let listed: Vec<_> = spaces
        .iter()
        .map(|space| (space.id.clone(), space.folders[0].clone().unwrap()))
        .collect();
    let expected: Vec<_> = app
        .spaces()
        .iter()
        .map(|space| (space.id.clone(), space.panes[0].live.clone()))
        .collect();
    assert_eq!(listed, expected);
}

#[test]
fn notifications_are_appended_as_json_lines() {
    let fixture = Fixture::new();

    fixture.backend.notify("herdr-kiosk", "first").unwrap();
    fixture
        .backend
        .notify("herdr-kiosk", "second \"quoted\"")
        .unwrap();

    let contents = fs::read_to_string(fixture.temp.path().join("notices/kiosk.jsonl")).unwrap();
    let bodies: Vec<String> = contents
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["body"].to_string())
        .collect();
    assert_eq!(bodies, [r#""first""#, r#""second \"quoted\"""#]);
}

fn editor_on_open() -> OnOpenConfig {
    OnOpenConfig {
        panes: vec![OnOpenPaneConfig {
            command: "hx".into(),
            direction: OnOpenPaneDirection::Right,
            ratio: None,
        }],
    }
}

#[test]
fn opening_a_repo_runs_on_open_panes_in_the_new_space() {
    let fixture = Fixture::with_on_open(editor_on_open());

    let opened = fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();

    assert_eq!(opened, Opened::default());
    let spaces = fixture.app.spaces();
    assert_eq!(spaces.len(), 1);
    let [root, editor] = spaces[0].panes.as_slice() else {
        panic!("expected two panes, got {:?}", spaces[0].panes);
    };
    assert_eq!(
        fixture.app.state.lock().unwrap().typed,
        [(editor.terminal.clone(), "hx".into())]
    );
    assert_eq!(
        kiosk_core::path::canonical_or_original(&editor.start),
        kiosk_core::path::canonical_or_original(&fixture.repo)
    );
    assert_eq!(
        fixture.app.state.lock().unwrap().focused_pane.as_ref(),
        Some(&root.pane)
    );
}

#[test]
fn an_on_open_pane_with_a_ratio_is_refused_with_a_warning() {
    let mut on_open = editor_on_open();
    on_open.panes[0].ratio = Some(0.3);
    let fixture = Fixture::with_on_open(on_open);

    let opened = fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();

    let warning = opened.warning.unwrap();
    assert!(warning.contains("always splits panes in half"), "{warning}");
    assert_eq!(fixture.app.spaces()[0].panes.len(), 1);
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
