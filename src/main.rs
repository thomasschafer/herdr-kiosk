use std::sync::Arc;

use anyhow::Result;
use kiosk_core::{
    config,
    git::{CliGitProvider, GitProvider},
};

use herdr_kiosk::{
    PRODUCT,
    config::Config,
    context::PluginContext,
    herdr::{CliHerdr, HerdrTui},
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
    let loaded = config::load_config::<Config>(&PRODUCT)?;
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
    }
    Ok(())
}
