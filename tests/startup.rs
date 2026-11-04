use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Write},
    net::{Ipv4Addr, TcpStream},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

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
fn an_unknown_command_exits_with_its_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-kiosk"))
        .arg("serve")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown command serve"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("usage: herdr-kiosk"),
        "stderr was: {stderr}"
    );
}

/// Stand-ins for `herdr-app` and `hc` with no spaces open, on a `PATH` of their own.
fn fake_app_programs(dir: &Path) -> OsString {
    let bin = dir.join("bin");
    fs::create_dir(&bin).unwrap();
    let spaces = r#"{"spaces":[]}"#;
    #[cfg(unix)]
    for name in ["herdr-app", "hc"] {
        use std::os::unix::fs::PermissionsExt;
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\nprintf '%s' '{spaces}'\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)]
    for name in ["herdr-app", "hc"] {
        fs::write(
            bin.join(format!("{name}.cmd")),
            format!("@echo {spaces}\r\n"),
        )
        .unwrap();
    }
    let mut path = vec![bin];
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(path).unwrap()
}

#[test]
fn the_app_command_serves_the_picker_in_the_background() {
    let temp = tempfile::tempdir().unwrap();
    let code = temp.path().join("code");
    fs::create_dir_all(code.join("alpha/.git")).unwrap();
    let config = temp.path().join("config/herdr-kiosk");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("config.toml"),
        format!("search_dirs = [{:?}]\n", code.display().to_string()),
    )
    .unwrap();

    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-kiosk"))
        .args(["app", "--context", r#"{"focused_pane_cwd":null}"#])
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("PATH", fake_app_programs(temp.path()))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the command returns once the picker listens"
    );
    let handshake: Value = serde_json::from_slice(&output.stdout).unwrap();

    let port = u16::try_from(handshake["port"].as_u64().unwrap()).unwrap();
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut send = |message: Value| {
        writer.write_all(format!("{message}\n").as_bytes()).unwrap();
    };
    let mut receive = || loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "connection closed"
        );
        let message: Value = serde_json::from_str(&line).unwrap();
        if message["t"] != "ping" {
            return message;
        }
    };
    send(json!({ "t": "hello", "token": handshake["token"], "protocol": handshake["protocol"] }));
    let hello = receive();
    assert_eq!(hello["t"], "hello", "{hello}");
    assert_eq!(hello["product"], "herdr-kiosk");
    let rows = loop {
        let message = receive();
        assert_eq!(message["t"], "view", "{message}");
        let rows = &message["view"]["main"]["rows"];
        if rows.as_array().is_some_and(|rows| !rows.is_empty()) {
            break rows.clone();
        }
    };
    assert_eq!(rows[0]["name"]["text"], "alpha");
    send(json!({ "t": "close" }));
    let end = loop {
        let message = receive();
        if message["t"] != "view" {
            break message;
        }
    };
    assert_eq!(end, json!({ "t": "exit", "outcome": "quit" }));
}
