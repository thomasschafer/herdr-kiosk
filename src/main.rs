use std::sync::Arc;

use anyhow::{Context, Result};
use kiosk_core::{
    backend::WorktreePlacement,
    config,
    git::{CliGitProvider, GitProvider},
};

use herdr_kiosk::{
    APP_PRODUCT, PRODUCT,
    config::Config,
    context::PluginContext,
    herdr::{
        CliHerdr, HerdrTui,
        app::{CliAppHost, HerdrApp, WORKTREE_ROOT},
    },
    launch::{self, AppArgs, Host, Invocation},
};

fn main() {
    if let Err(error) = start() {
        eprintln!("{}: {error:#}", PRODUCT.name);
        std::process::exit(1);
    }
}

fn start() -> Result<()> {
    match launch::invocation()? {
        Invocation::Terminal(host) => terminal(&host),
        Invocation::App(args) => kiosk_serve::serve(&APP_PRODUCT, |notifier| app(args, notifier)),
    }
}

fn terminal(host: &Host) -> Result<()> {
    let loaded = config::load_config::<Config>(&PRODUCT)?;
    let on_open = loaded.extension.on_open.clone();
    let git = Arc::new(CliGitProvider) as Arc<dyn GitProvider>;
    let current_cwd = host.context().and_then(PluginContext::current_cwd);
    match host {
        Host::Standalone => {
            kiosk_tui::launch::run::<HerdrTui<CliHerdr>, _>(
                &PRODUCT,
                loaded,
                git,
                None,
                current_cwd,
            )?;
        }
        Host::HerdrTui { herdr_binary, .. } => {
            let backend = HerdrTui::new(CliHerdr::new(herdr_binary), on_open);
            kiosk_tui::launch::run(&PRODUCT, loaded, git, Some(Arc::new(backend)), current_cwd)?;
        }
    }
    Ok(())
}

fn app(
    args: AppArgs,
    notifier: kiosk_serve::Notifier,
) -> Result<kiosk_serve::Start<HerdrApp, Config>> {
    let loaded = config::load_config::<Config>(&APP_PRODUCT)?;
    let home = kiosk_core::path::home_dir()
        .context("HOME must be set to place new worktrees for the Herdr app")?;
    let placement = WorktreePlacement::new(home.join(WORKTREE_ROOT)).map_err(anyhow::Error::msg)?;
    let host = CliAppHost::new(
        launch::find_program("herdr-app")?,
        launch::find_program("hc")?,
        args.app_home,
    );
    let git = Arc::new(CliGitProvider) as Arc<dyn GitProvider>;
    let backend = HerdrApp::new(
        Arc::new(host),
        Arc::clone(&git),
        placement,
        Arc::new(notifier),
        loaded.extension.on_open.clone(),
    );
    Ok(kiosk_serve::Start {
        current_cwd: args.context.current_cwd().map(ToOwned::to_owned),
        loaded,
        git,
        backend: Some(Arc::new(backend)),
    })
}
