//! The profile setup script, run for real under `sh` in a scratch home.

use std::fs;
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
