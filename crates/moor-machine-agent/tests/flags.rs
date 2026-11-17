//! The agent's command line refuses flag combinations that mean nothing.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "tests")]

use std::process::Command;

/// Runs the agent with `args` in an empty home: whether it succeeded, and its stderr.
fn agent(args: &[&str]) -> (bool, String) {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_moor-machine-agent"))
        .args(args)
        .env("HOME", home.path())
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap();
    (out.status.success(), String::from_utf8(out.stderr).unwrap())
}

#[test]
fn the_crabbox_flags_are_unknown() {
    for flag in ["--runtime", "--crabbox", "--cache-gb", "--max-boxes"] {
        let (ok, err) = agent(&["run", flag, "x"]);
        assert!(!ok);
        assert!(err.contains("unexpected argument"), "{flag}: {err}");
    }
}

#[test]
fn a_machine_that_runs_exec_provides_no_boxes() {
    let (ok, err) = agent(&["run", "--boxes", "docker", "--allow-exec"]);
    assert!(!ok);
    assert!(err.contains("--allow-exec"), "{err}");
}
