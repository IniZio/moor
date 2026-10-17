//! `box.host_steps` parse, validation and platform check.
#![allow(clippy::panic, reason = "test assertions")]

use super::{parse_repo, HostStep};

/// Asserts the document is rejected with `path` and a message containing `needle`.
fn rejects(text: &str, path: &str, needle: &str) {
    match parse_repo("m.yaml", text) {
        Ok(_) => panic!("accepted: {text}"),
        Err(e) => {
            assert_eq!(e.path, path, "{text}: {e}");
            assert!(e.to_string().contains(needle), "{text}: {e}");
        }
    }
}

#[test]
fn host_steps_parse() {
    let cfg =
        parse_repo("m.yaml", "box: {host_steps: [backend]}").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(cfg.box_.host_steps, [HostStep::Backend]);
    assert!(cfg.box_.wants_backend());
}

#[test]
fn host_steps_absent_is_empty() {
    let cfg = parse_repo("m.yaml", "box: {}").unwrap_or_else(|e| panic!("{e}"));
    assert!(cfg.box_.host_steps.is_empty());
    assert!(!cfg.box_.wants_backend());
}

#[test]
fn unknown_host_step_fails() {
    rejects(
        "box: {host_steps: [backend, db]}",
        "box.host_steps[1]",
        "unknown host step \"db\"; the only host step is \"backend\"",
    );
}

#[test]
fn duplicate_host_step_fails() {
    rejects(
        "box: {host_steps: [backend, backend]}",
        "box.host_steps[1]",
        "list each host step once",
    );
}

#[test]
fn host_steps_refused_off_macos() {
    let cfg =
        parse_repo("m.yaml", "box: {host_steps: [backend]}").unwrap_or_else(|e| panic!("{e}"));
    assert!(cfg.box_.check_platform(true).is_ok());
    let e = cfg.box_.check_platform(false).expect_err("refused");
    assert_eq!(e.path, "box.host_steps");
    assert!(e.to_string().contains("applies only to a macOS Box"), "{e}");
}
