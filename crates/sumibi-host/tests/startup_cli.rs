#![cfg(windows)]
use std::process::Command;

#[test]
fn startup_check_rejects_relative_paths_without_echoing_arguments() {
    let output = Command::new(env!("CARGO_BIN_EXE_sumibi-host"))
        .args(["startup-check", "private-settings-name.json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "startup settings unavailable or invalid"
    );
}

#[test]
fn startup_check_rejects_extra_arguments_before_loading() {
    let output = Command::new(env!("CARGO_BIN_EXE_sumibi-host"))
        .args(["startup-check", "private-settings-name.json", "unexpected"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "usage: sumibi-host startup-check [absolute-settings-path]"
    );
}

#[test]
fn settings_init_creates_resolved_settings_and_refuses_to_overwrite() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("sumibi-settings-cli-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("settings.json");
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0.join("settings.json"));
            let _ = std::fs::remove_file(self.0.join(".settings.json.lock"));
            let _ = std::fs::remove_dir(&self.0);
        }
    }
    let _cleanup = Cleanup(directory);
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_sumibi-host"))
            .args(["settings-init", "--settings"])
            .arg(&path)
            .args(["example.test", "fixture-model", "/v1"])
            .output()
            .unwrap()
    };
    let created = run();
    assert!(created.status.success());
    assert_eq!(
        String::from_utf8(created.stdout).unwrap().trim(),
        "settings created; sending requires explicit consent"
    );
    let loaded = sumibi_host::settings::Settings::load(&path).unwrap();
    assert_eq!(loaded.model(), "fixture-model");
    assert_eq!(loaded.destination().base_path(), "/v1/chat/completions");
    let bytes = std::fs::read(&path).unwrap();
    let repeated = run();
    assert_eq!(repeated.status.code(), Some(1));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    // A malformed existing file is also preserved for explicit repair.
    std::fs::write(&path, b"invalid record").unwrap();
    assert_eq!(run().status.code(), Some(1));
    assert_eq!(std::fs::read(&path).unwrap(), b"invalid record");
}

#[test]
fn host_run_rejects_extra_arguments() {
    let output = Command::new(env!("CARGO_BIN_EXE_sumibi-host"))
        .args(["run", "private-settings-name.json", "unexpected"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "usage: sumibi-host run [absolute-settings-path]"
    );
}

#[test]
fn host_run_fails_with_invalid_settings() {
    let output = Command::new(env!("CARGO_BIN_EXE_sumibi-host"))
        .args(["run", "Z:\\nonexistent\\settings.json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("settings invalid or unavailable"));
}

#[test]
fn host_cli_help_displays_usage_and_commands() {
    for flag in ["--help", "-h", "help"] {
        let output = Command::new(env!("CARGO_BIN_EXE_sumibi-host"))
            .arg(flag)
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("Sumibi Host service and settings management"));
        assert!(stdout.contains("Usage: sumibi-host [command] [options...]"));
        assert!(stdout.contains("host-window"));
        assert!(stdout.contains("settings-window"));
        assert!(stdout.contains("run"));
        assert!(stdout.contains("--minimized"));
    }
}

#[test]
fn host_cli_help_displays_minimized_option() {
    let output = Command::new(env!("CARGO_BIN_EXE_sumibi-host"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--minimized, --tray, -m"));
}
