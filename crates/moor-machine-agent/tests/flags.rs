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
fn a_cache_cap_needs_a_runtime() {
    let (ok, err) = agent(&["run", "--cache-gb", "5"]);
    assert!(!ok);
    assert!(err.contains("--runtime"), "{err}");
}

#[test]
fn a_runtime_host_does_not_run_exec() {
    let (ok, err) = agent(&["run", "--runtime", "x", "--allow-exec"]);
    assert!(!ok);
    assert!(err.contains("--allow-exec"), "{err}");
}

#[test]
fn the_old_pool_flags_are_unknown() {
    for flag in ["--pool", "--pool-max"] {
        let (ok, err) = agent(&["run", flag, "x"]);
        assert!(!ok);
        assert!(err.contains("unexpected argument"), "{flag}: {err}");
    }
}
