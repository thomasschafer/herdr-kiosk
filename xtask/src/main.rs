use std::{env, path::Path};

use anyhow::{Result, bail};
use herdr_kiosk::config::{Config, SOURCE};
use kiosk_docgen::ProductDocs;

const DOCS: ProductDocs<'static> = ProductDocs {
    sources: &[SOURCE],
    extension: "Config",
    order: &["search_dirs", "include_non_git", "theme", "on_open", "keys"],
    examples: &[(
        "on_open",
        "[on_open]\npanes = [\n  { command = \"hx\", direction = \"right\" },\n]",
    )],
    wording: &[
        (
            "OnOpenConfig::panes",
            "Pane definitions, created in order without moving focus from the primary\n\
             pane. Commands run from the opened repository or worktree. They run only\n\
             when a workspace is newly opened, not when an existing workspace is focused.",
        ),
        (
            "OnOpenPaneConfig",
            "A command pane created after a new workspace is opened.",
        ),
        (
            "OnOpenPaneConfig::command",
            "Shell command Herdr runs in the opened checkout. The command must not be empty.",
        ),
        (
            "OnOpenPaneConfig::ratio",
            "Fraction of the resulting split occupied by the new command pane. The value must be \
             greater than 0 and less than 1, and defaults to 0.5 when omitted. The Herdr app \
             always splits in half, so it refuses a pane with a ratio.",
        ),
    ],
};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let Some(task) = args.next() else {
        bail!("usage: cargo xtask readme [--check]");
    };
    if task != "readme" {
        bail!("unknown xtask '{task}'; expected 'readme'");
    }

    let mut check = false;
    for argument in args {
        if argument == "--check" && !check {
            check = true;
        } else {
            bail!("unexpected argument '{argument}'; usage: cargo xtask readme [--check]");
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace parent");
    kiosk_docgen::generate::<Config>(&root.join("README.md"), &DOCS, check)
}
