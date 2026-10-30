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
        "usage: sumibi-host startup-check <absolute-settings-path>"
    );
}
