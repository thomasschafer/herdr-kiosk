use std::{error::Error, fs, io, path::PathBuf};

use herdr_kiosk::config::keys::default_keys_toml;

const KEYS_START: &str = "<!-- KEYS:START -->";
const KEYS_END: &str = "<!-- KEYS:END -->";

fn main() -> Result<(), Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let readme = fs::read_to_string(&path)?;
    let (before, after_start) = readme
        .split_once(KEYS_START)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing keys start marker"))?;
    let (_, after) = after_start
        .split_once(KEYS_END)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing keys end marker"))?;
    let updated = format!(
        "{before}{KEYS_START}\n{}{KEYS_END}{after}",
        default_keys_toml()
    );
    if updated != readme {
        fs::write(path, updated)?;
    }
    Ok(())
}
