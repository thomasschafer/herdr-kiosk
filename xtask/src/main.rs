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
