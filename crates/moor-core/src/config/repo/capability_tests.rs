//! `box.capabilities` and `box.sandbox` parse, validation and platform check.
#![allow(clippy::panic, reason = "test assertions")]

use super::{parse_repo, BoxCapability, RepoConfig};

fn ok(text: &str) -> RepoConfig {
    parse_repo("m.yaml", text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

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
fn capabilities_parse() {
    assert_eq!(
        ok("box: {capabilities: [xcode]}").box_.capabilities,
        [BoxCapability::Xcode]
    );
    assert_eq!(
        ok("box: {capabilities: [ios-simulator]}").box_.capabilities,
        [BoxCapability::IosSimulator]
    );
}

#[test]
fn unknown_capability_fails() {
    assert!(parse_repo("m.yaml", "box: {capabilities: [gpu]}").is_err());
}

#[test]
fn duplicate_capability_is_refused() {
    rejects(
        "box: {capabilities: [xcode, xcode]}",
        "box.capabilities[1]",
        "once",
    );
}

#[test]
fn relative_sandbox_paths_pass() {
    let c = ok("box:\n  sandbox:\n    read: [docs, a/b]\n    write: [build/out]\n");
    assert_eq!(c.box_.sandbox.read, ["docs", "a/b"]);
    assert_eq!(c.box_.sandbox.write, ["build/out"]);
}

#[test]
fn outside_sandbox_paths_are_refused() {
    for bad in ["/etc", "~/x", "../x", "a/../../x", "\"\""] {
        for kind in ["read", "write"] {
            rejects(
                &format!("box:\n  sandbox:\n    {kind}: [ok, {bad}]\n"),
                &format!("box.sandbox.{kind}[1]"),
                "inside the workdir",
            );
        }
    }
}

#[test]
fn check_platform_both_ways() {
    let c = ok("box: {capabilities: [xcode]}");
    assert!(c.box_.check_platform(true).is_ok());
    let e = c.box_.check_platform(false).expect_err("not macOS");
    assert_eq!(e.path, "box.capabilities");
    assert!(e.to_string().contains("applies only to a macOS Box"), "{e}");
    assert!(ok("box: {size: small}").box_.check_platform(false).is_ok());
}

#[test]
fn changed_keys_reports_box_for_capabilities() {
    let a = ok("box: {capabilities: [xcode]}");
    let b = ok("box: {capabilities: [xcode, ios-simulator]}");
    assert_eq!(a.changed_keys(&b), ["box"]);
}
