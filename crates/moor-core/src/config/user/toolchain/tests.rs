//! Tests for the host toolchain table and name selection.

use super::*;
use crate::config::parse_user;

fn table(doc: &str) -> ToolchainTable {
    let r = parse_user("u.yaml", doc);
    assert!(r.is_ok(), "{doc}: {r:?}");
    r.unwrap_or_default().toolchains
}

fn err(doc: &str) -> ConfigError {
    parse_user("u.yaml", doc).expect_err(doc)
}

const MISE: &str = "toolchains:
  mise:
    root: /Users/me/.local/share/mise
    env: {MISE_DATA_DIR: \"{root}\", B: \"{root}/x:{root}\"}
    bin: [installs/node/22/bin, shims]
  rbenv: {root: /Users/me/.rbenv}
";

#[test]
fn parses_table() {
    let t = table(MISE);
    assert_eq!(t.len(), 2);
    assert_eq!(t.get("rbenv").map(|x| x.env.len()), Some(0));
    assert_eq!(
        t.get("mise").map(|x| x.bin.clone()),
        Some(vec![
            "installs/node/22/bin".to_string(),
            "shims".to_string()
        ])
    );
}

#[test]
fn absent_table_is_empty() {
    assert!(table("").is_empty());
}

#[test]
fn unknown_field_refused() {
    err("toolchains: {a: {root: /x/y, extra: 1}}");
}

#[test]
fn select_keeps_names_order_and_expands() {
    let t = table(MISE);
    let names = ["rbenv".to_string(), "mise".to_string()];
    let got = select_toolchains(&t, &names).unwrap();
    let (Some(rbenv), Some(mise)) = (got.first(), got.get(1)) else {
        return assert!(got.len() == 2, "two toolchains expected: {got:?}");
    };
    assert_eq!(rbenv.name, "rbenv");
    assert_eq!(mise.name, "mise");
    assert_eq!(mise.root, "/Users/me/.local/share/mise");
    assert_eq!(
        mise.bin,
        [
            "/Users/me/.local/share/mise/installs/node/22/bin",
            "/Users/me/.local/share/mise/shims"
        ]
    );
    let root = "/Users/me/.local/share/mise";
    assert_eq!(
        mise.env,
        [
            ("B".to_string(), format!("{root}/x:{root}")),
            ("MISE_DATA_DIR".to_string(), root.to_string()),
        ]
    );
}

#[test]
fn unknown_name_message() {
    let t = table(MISE);
    let e = select_toolchains(&t, &["mise".into(), "nope".into()]).unwrap_err();
    assert_eq!(e.path, "box.toolchains[1]");
    assert_eq!(
        e.reason,
        "unknown toolchain \"nope\"; the host config names: mise, rbenv"
    );
    let e = select_toolchains(&ToolchainTable::new(), &["x".into()]).unwrap_err();
    assert_eq!(
        e.reason,
        "unknown toolchain \"x\"; the host config names none"
    );
}

fn bad(entry: &str, path: &str) {
    let e = err(&format!("toolchains:\n  {entry}"));
    assert_eq!(e.path, path, "{entry}: {e}");
}

#[test]
fn refuses_bad_names() {
    bad("Mise: {root: /a/b}", "toolchains.Mise");
    bad("\"\": {root: /a/b}", "toolchains.");
    bad("\"a b\": {root: /a/b}", "toolchains.a b");
}

#[test]
fn refuses_bad_roots() {
    for r in [
        "rel/x",
        "/",
        "/a/./b",
        "/a/../b",
        "/a/b/",
        "\"/a/\\\"b\"",
        "\"/a/\\\\b\"",
        "\"/a/\\nb\"",
        "\"\"",
    ] {
        bad(&format!("t: {{root: {r}}}"), "toolchains.t.root");
    }
}

#[test]
fn refuses_bad_env() {
    bad("t: {root: /a/b, env: {\"1X\": v}}", "toolchains.t.env.1X");
    for n in [
        "PATH",
        "HOME",
        "TMPDIR",
        "CFFIXED_USER_HOME",
        "XCODE_XCCONFIG_FILE",
        "MOOR_BOX_TAG",
        "DEVELOPER_DIR",
    ] {
        bad(
            &format!("t: {{root: /a/b, env: {{{n}: v}}}}"),
            &format!("toolchains.t.env.{n}"),
        );
    }
    let e = err("toolchains: {t: {root: /a/b, env: {PATH: v}}}");
    assert!(e.reason.contains("PATH entries go in bin"), "{e}");
}

#[test]
fn refuses_bad_bin() {
    for b in ["\"\"", "/abs", "a/../b", ".."] {
        bad(
            &format!("t: {{root: /a/b, bin: [{b}]}}"),
            "toolchains.t.bin[0]",
        );
    }
}
