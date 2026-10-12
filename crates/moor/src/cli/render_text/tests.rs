//! Tests for text rendering.

use moor_core::config::Layer;
use moor_core::view::env::{
    EnvAgent, EnvCredential, EnvEgress, EnvGithub, EnvKind, EnvMint, EnvProvider, EnvResolved,
    EnvRow, EnvView, ValueKind,
};
use moor_core::{ErrorKind, ErrorView, Response, Warning};
use serde_json::json;

use super::render;

/// Builds a warning with `msg`.
fn warn(msg: &str) -> Warning {
    Warning {
        code: "w".into(),
        message: msg.into(),
    }
}

#[test]
fn version_line_is_exact() {
    let hash = "a".repeat(40);
    let v = json!({"version": "0.2.0", "commit": hash, "schema": 1});
    let (out, err) = render(&Response::ok("version", Some(v)), false);
    assert_eq!(out, format!("moor 0.2.0 {hash} schema=1\n"));
    assert_eq!(err, "");
}

#[test]
fn odd_version_value_falls_back_to_json() {
    let (out, _) = render(&Response::ok("version", Some(json!({"version": 3}))), false);
    assert!(out.contains("\"version\": 3"));
    assert!(out.ends_with('\n'));
}

#[test]
fn cp_prints_its_confirmation() {
    let v = json!({"copied": true, "confirmation": "copied a to b (3 bytes)"});
    let (out, _) = render(&Response::ok("cp", Some(v)), false);
    assert_eq!(out, "copied a to b (3 bytes)\n");
}

#[test]
fn other_value_is_pretty_json_and_none_is_empty() {
    let (out, _) = render(&Response::ok("show", Some(json!({"a": 1}))), false);
    assert_eq!(out, "{\n  \"a\": 1\n}\n");
    assert_eq!(
        render(&Response::ok("show", None), false),
        (String::new(), String::new())
    );
}

#[test]
fn error_prints_message_and_hints() {
    let e = ErrorView {
        kind: ErrorKind::Usage,
        message: "bad thing".into(),
        hints: vec!["try x".into(), "or y".into()],
        context: vec![],
    };
    let (out, err) = render(&Response::err("show", e), false);
    assert_eq!(out, "");
    assert_eq!(err, "moor: bad thing\nhint: try x\nhint: or y\n");
}

#[test]
fn warnings_print_unless_quiet() {
    let resp = Response::ok("show", None).with_warnings(vec![warn("careful")]);
    assert_eq!(render(&resp, false).1, "moor: warning: careful\n");
    assert_eq!(render(&resp, true).1, "");
}

/// Builds a row.
fn row(kind: EnvKind, name: &str, source: Layer, note: &str) -> EnvRow {
    EnvRow {
        kind,
        name: name.into(),
        source,
        value_kind: ValueKind::Plain,
        delivery: None,
        note: note.into(),
    }
}

/// Builds a credential check.
fn check(name: &str, ok: bool, reason: Option<&str>) -> EnvCredential {
    EnvCredential {
        kind: EnvKind::Credential,
        which: name.into(),
        ok,
        reason: reason.map(Into::into),
    }
}

/// Builds an env response from `rows` and `credentials`.
fn env_view(rows: Vec<EnvRow>, credentials: Vec<EnvCredential>) -> Response {
    let github = EnvGithub {
        identity: "auto".into(),
        app_id: None,
        login: "logged_out".into(),
        mint: EnvMint {
            status: "skipped".into(),
            error: None,
            reason: Some("GitHub App not set up".into()),
        },
        resolved: EnvResolved {
            mode: None,
            app_installed: "n/a".into(),
            token_source: None,
            error: Some("no app".into()),
        },
    };
    let view = EnvView {
        repo: "/r".into(),
        agent: EnvAgent { model: None },
        github,
        rows,
        egress: EnvEgress {
            mode: "allow".into(),
            hosts: vec![],
            note: String::new(),
            enforce: None,
        },
        provider: EnvProvider {
            name: "local".into(),
            source: Layer::Default,
            size: "small".into(),
            size_source: Layer::Default,
        },
        credentials,
    };
    Response::ok("env", Some(serde_json::to_value(view).unwrap()))
}

#[test]
fn env_text_starts_with_agent_and_marks_credentials() {
    let rows = vec![
        row(EnvKind::Credential, "GOOD", Layer::User, "env: X"),
        row(EnvKind::Credential, "BAD", Layer::Repo, "file: /x"),
        row(EnvKind::Egress, "allow", Layer::Default, "2 hosts"),
    ];
    let credentials = vec![
        check("GOOD", true, None),
        check("BAD", false, Some("missing")),
    ];
    let (out, _) = render(&env_view(rows, credentials), false);
    assert!(out.starts_with("agent"));
    let lines: Vec<&str> = out.lines().collect();
    let good = lines.iter().find(|l| l.contains("GOOD")).unwrap();
    assert!(good.contains('\u{2713}') && !good.contains('\u{2717}'));
    let bad = lines.iter().find(|l| l.contains("BAD")).unwrap();
    assert!(bad.contains('\u{2717}') && !bad.contains('\u{2713}'));
    assert!(bad.ends_with("(missing)"));
    assert!(out.contains("github token: skipped: GitHub App not set up"));
    assert!(out.contains("github identity: unknown: no app"));
    assert!(lines
        .last()
        .unwrap()
        .starts_with("- egress allow  [default]  2 hosts"));
}
