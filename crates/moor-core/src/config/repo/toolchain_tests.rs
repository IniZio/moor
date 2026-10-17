//! `box.toolchains` parse, validation and platform check.
#![allow(clippy::panic, reason = "test assertions")]

use super::parse_repo;

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
fn toolchains_parse() {
    let cfg = parse_repo("m.yaml", "box: {toolchains: [mise, flutter-3.x, a_b]}")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(cfg.box_.toolchains, ["mise", "flutter-3.x", "a_b"]);
    assert!(parse_repo("m.yaml", "box: {}")
        .unwrap_or_else(|e| panic!("{e}"))
        .box_
        .toolchains
        .is_empty());
}

#[test]
fn toolchains_refused_off_macos() {
    let cfg = parse_repo("m.yaml", "box: {toolchains: [mise]}").unwrap_or_else(|e| panic!("{e}"));
    assert!(cfg.box_.check_platform(true).is_ok());
    let e = cfg.box_.check_platform(false).expect_err("refused");
    assert_eq!(e.path, "box.toolchains");
    assert!(e.to_string().contains("applies only to a macOS Box"), "{e}");
}

#[test]
fn duplicate_toolchain_fails() {
    rejects(
        "box: {toolchains: [mise, rbenv, mise]}",
        "box.toolchains[2]",
        "list each toolchain once",
    );
}

#[test]
fn malformed_toolchain_name_fails() {
    for bad in ["''", "Mise", "\"a b\"", "\"a/b\"", "\"\\\\x\""] {
        let text = format!("box: {{toolchains: [{bad}]}}");
        rejects(&text, "box.toolchains[0]", "toolchain name");
    }
}

#[test]
fn toolchain_mapping_fails_to_parse() {
    let text = "box: {toolchains: [{root: /x}]}";
    assert!(parse_repo("m.yaml", text).is_err());
}
