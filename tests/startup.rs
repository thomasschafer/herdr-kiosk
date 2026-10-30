use std::{fs, process::Command};

#[test]
fn invalid_config_exits_nonzero_with_a_clear_error() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("config.toml"), "search_dirs = 42").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_herdr-kiosk"))
        .env("HERDR_PLUGIN_CONFIG_DIR", temp.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("herdr-kiosk: "), "stderr was: {stderr}");
    assert!(
        stderr.contains("invalid config TOML"),
        "stderr was: {stderr}"
    );
    assert!(!stderr.contains("panicked"), "stderr was: {stderr}");
}

#[test]
fn herdr_tui_and_herdr_app_variables_together_exit_with_a_clear_error() {
    let temp = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_herdr-kiosk"))
        .env("XDG_CONFIG_HOME", temp.path())
        .env("HERDR_BIN_PATH", "/bin/herdr")
        .env("HERDR_KIOSK_APP_BIN", "/bin/herdr-app")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("herdr-kiosk: both "),
        "stderr was: {stderr}"
    );
    assert!(!stderr.contains("panicked"), "stderr was: {stderr}");
}
