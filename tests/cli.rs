use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_imap-notify"))
        .env_remove("RUST_LOG")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_version_and_offline_check() {
    for arg in ["--help", "--version"] {
        assert!(run(&[arg]).status.success());
    }
    let output = run(&["--version"]);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("imap-notify {}", env!("CARGO_PKG_VERSION"))
    );
    let config = format!("{}/config.example.toml", env!("CARGO_MANIFEST_DIR"));
    assert!(run(&["-c", &config, "--check"]).status.success());
    for args in [
        vec!["--unknown"],
        vec!["-c"],
        vec!["--check", "--test-bark"],
        vec!["-c", "/nonexistent/imap-notify.toml"],
    ] {
        assert!(!run(&args).status.success());
    }
}

#[test]
fn malformed_config_diagnostics_hide_source_secrets() {
    let path = std::env::temp_dir().join(format!("imap-notify-cli-{}.toml", std::process::id()));
    std::fs::write(
        &path,
        "[bark]\nurl = \"https://api.day.app/private-device-key\" trailing\n",
    )
    .unwrap();
    let output = run(&["-c", path.to_str().unwrap(), "--check"]);
    std::fs::remove_file(path).unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-device-key"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-device-key"));
}
