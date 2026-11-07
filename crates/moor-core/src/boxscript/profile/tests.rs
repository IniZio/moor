//! The profile setup script, run for real under `sh` in a scratch home.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

use super::{setup_script, PROFILE_DIR};

/// Runs the setup script with `HOME` at a fresh dir holding a checkout with `files`.
fn run(files: &[(&str, &str)]) -> Output {
    let home = tempfile::tempdir().unwrap();
    let checkout = home.path().join(PROFILE_DIR);
    fs::create_dir_all(&checkout).unwrap();
    for (name, text) in files {
        fs::write(checkout.join(name), text).unwrap();
    }
    Command::new("sh")
        .arg("-c")
        .arg(setup_script())
        .env("HOME", home.path())
        .output()
        .unwrap()
}

#[test]
fn a_checkout_without_setup_sh_exits_clean_and_runs_nothing() {
    let out = run(&[("AGENTS.md", "hi")]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

#[test]
fn setup_sh_runs_from_the_checkout() {
    let out = run(&[("setup.sh", "pwd\n")]);
    assert!(out.status.success());
    let printed = String::from_utf8(out.stdout).unwrap();
    assert!(printed.trim_end().ends_with(PROFILE_DIR), "{printed}");
}

#[test]
fn setup_sh_failure_is_the_scripts_exit_code() {
    let out = run(&[("setup.sh", "exit 7\n")]);
    assert_eq!(out.status.code(), Some(7));
}

#[test]
fn a_missing_checkout_fails() {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new("sh")
        .arg("-c")
        .arg(setup_script())
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn setup_sh_finds_a_binary_moor_put_in_the_home_bin() {
    let home = tempfile::tempdir().unwrap();
    let checkout = home.path().join(PROFILE_DIR);
    fs::create_dir_all(&checkout).unwrap();
    fs::write(checkout.join("setup.sh"), "moor-test-tool hello\n").unwrap();
    let bin = home.path().join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    let tool = bin.join("moor-test-tool");
    fs::write(&tool, "#!/bin/sh\necho \"tool $*\"\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new("sh")
        .arg("-c")
        .arg(setup_script())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "tool hello\n");
}

#[test]
fn failed_installs_are_the_lines_after_the_prefix() {
    let output = "noise\nmoor: profile: plugin a exit 7\nmoor: profile: marketplace b exit 1\n";
    assert_eq!(
        super::failed_installs(output),
        ["plugin a exit 7", "marketplace b exit 1"]
    );
}
