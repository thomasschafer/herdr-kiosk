use std::sync::Arc;

use anyhow::Result;
use kiosk_core::{
    backend::WorktreePlacement,
    config,
    git::{CliGitProvider, GitProvider},
};

use herdr_kiosk::{
    PRODUCT,
    context::PluginContext,
    herdr::{
        CliHerdr, HerdrTui,
        app::{CliAppHost, HerdrApp, WORKTREE_ROOT},
    },
    launch::{self, Host},
};

fn main() {
    if let Err(error) = start() {
        eprintln!("{}: {error:#}", PRODUCT.name);
        std::process::exit(1);
    }
}

fn start() -> Result<()> {
    let host = launch::detect_host()?;
    let loaded = config::load_config::<herdr_kiosk::config::Config>(&PRODUCT)?;
    let on_open = loaded.extension.on_open.clone();
    let git = Arc::new(CliGitProvider) as Arc<dyn GitProvider>;
    let current_cwd = host.context().and_then(PluginContext::current_cwd);
    match &host {
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
        Host::HerdrApp {
            herdr_app_binary,
            hc_binary,
            notice_file,
            ..
        } => {
            let Some(home) = kiosk_core::path::home_dir() else {
                anyhow::bail!("HOME must be set to place new worktrees for the Herdr app");
            };
            let placement =
                WorktreePlacement::new(home.join(WORKTREE_ROOT)).map_err(anyhow::Error::msg)?;
            let backend = HerdrApp::new(
                Arc::new(CliAppHost::new(herdr_app_binary, hc_binary)),
                Arc::clone(&git),
                placement,
                notice_file.clone(),
                on_open,
            );
            kiosk_tui::launch::run(&PRODUCT, loaded, git, Some(Arc::new(backend)), current_cwd)?;
        }
    }
    Ok(())
}
