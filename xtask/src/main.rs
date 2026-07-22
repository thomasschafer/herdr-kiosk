mod readme;

use std::{env, path::Path};

use anyhow::{Result, bail};

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
    readme::generate(
        &root.join("README.md"),
        &root.join("src/config/mod.rs"),
        check,
    )
}
