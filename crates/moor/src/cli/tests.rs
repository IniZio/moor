//! Tests for the cli module.

use std::ffi::OsString;

use moor_core::{Request, Response};

use super::{inherit_run_output, parse, parse_call, render, GlobalOpts, Parsed};

/// Builds argv with a program name.
fn argv(a: &[&str]) -> Vec<OsString> {
    std::iter::once("moor")
        .chain(a.iter().copied())
        .map(OsString::from)
        .collect()
}

/// Unwraps a `Done` parse result.
fn done(a: &[&str]) -> super::Output {
    let Parsed::Done(o) = parse(argv(a)) else {
        unreachable!("expected Done")
    };
    o
}

#[test]
fn json_usage_error_is_one_object() {
    let o = done(&["--json", "pane", "x"]);
    assert_eq!(o.code, 2);
    assert!(o.stderr.is_empty());
    let v: serde_json::Value = serde_json::from_str(&o.stdout).expect("one json object");
    assert_eq!(v.get("class"), Some(&serde_json::json!("usage")));
    assert_eq!(v.get("cmd"), Some(&serde_json::json!("pane")));
    assert_eq!(o.stdout.matches('\n').count(), 1);
}

#[test]
fn text_usage_error_goes_to_stderr() {
    let o = done(&["pane", "x"]);
    assert!(o.stdout.is_empty());
    assert!(!o.stderr.is_empty());
    assert_eq!(o.code, 2);
}

#[test]
fn help_exits_zero() {
    let o = done(&["--help"]);
    assert_eq!(o.code, 0);
    assert!(!o.stdout.is_empty());
}

#[test]
fn proxy_serve_is_failed_json() {
    let o = done(&["--json", "proxy", "serve"]);
    let v: serde_json::Value = serde_json::from_str(&o.stdout).expect("json");
    assert_eq!(v.get("class"), Some(&serde_json::json!("failed")));
}

#[test]
fn version_parses_to_request() {
    assert!(matches!(
        parse(argv(&["version"])),
        Parsed::Request(inv) if inv.request == Request::Version {}
    ));
}

#[test]
fn call_parses_stdin_mode() {
    assert!(matches!(parse(argv(&["call"])), Parsed::CallStdin(o) if o.json));
}

#[test]
fn parse_call_good_and_bad() {
    assert_eq!(
        parse_call(r#"{"cmd":"version"}"#).expect("ok"),
        Request::Version {}
    );
    let o = parse_call("{").expect_err("bad json");
    let v: serde_json::Value = serde_json::from_str(&o.stdout).expect("json");
    assert_eq!(v.get("class"), Some(&serde_json::json!("usage")));
    assert_eq!(v.get("cmd"), Some(&serde_json::json!("call")));
    assert_eq!(o.code, 2);
}

#[test]
fn render_json_version_is_single_line() {
    let resp = Response::ok(
        "version",
        Some(serde_json::json!({"version": "0", "commit": "c", "schema": 1})),
    );
    let opts = GlobalOpts {
        json: true,
        ..GlobalOpts::default()
    };
    let o = render(&resp, &opts);
    assert!(o.stdout.ends_with('\n'));
    assert_eq!(o.stdout.matches('\n').count(), 1);
    assert_eq!(o.code, 0);
}

#[test]
fn json_help_is_one_ok_envelope() {
    let o = done(&["--json", "--help"]);
    assert_eq!(o.code, 0);
    assert!(o.stderr.is_empty());
    assert_eq!(o.stdout.matches('\n').count(), 1);
    let v: serde_json::Value = serde_json::from_str(&o.stdout).expect("one json object");
    assert_eq!(v.get("class"), Some(&serde_json::json!("ok")));
    let text = v
        .get("value")
        .and_then(|x| x.as_str())
        .expect("string value");
    assert!(text.contains("Usage"));
}

#[test]
fn json_subcommand_help_is_one_ok_envelope() {
    let o = done(&["--json", "version", "--help"]);
    assert_eq!(o.code, 0);
    assert!(o.stderr.is_empty());
    let v: serde_json::Value = serde_json::from_str(&o.stdout).expect("one json object");
    assert_eq!(v.get("class"), Some(&serde_json::json!("ok")));
    assert_eq!(v.get("cmd"), Some(&serde_json::json!("version")));
}

#[test]
fn json_without_subcommand_is_usage_envelope() {
    let o = done(&["--json"]);
    assert_eq!(o.code, 2);
    assert!(o.stderr.is_empty());
    assert_eq!(o.stdout.matches('\n').count(), 1);
    let v: serde_json::Value = serde_json::from_str(&o.stdout).expect("one json object");
    assert_eq!(v.get("class"), Some(&serde_json::json!("usage")));
}

#[test]
fn text_help_stays_plain() {
    let o = done(&["--help"]);
    assert!(o.stdout.contains("Usage"));
    assert!(serde_json::from_str::<serde_json::Value>(&o.stdout).is_err());
}

/// Parses a JSON usage envelope from a `Done` result.
fn usage_json(a: &[&str]) -> serde_json::Value {
    let o = done(a);
    assert_eq!(o.code, 2, "{a:?}: {o:?}");
    assert!(o.stderr.is_empty());
    let v: serde_json::Value = serde_json::from_str(&o.stdout).expect("one json object");
    assert_eq!(v.get("class"), Some(&serde_json::json!("usage")));
    v
}

#[test]
fn state_dir_equals_form_keeps_cmd() {
    for a in [
        &["--state-dir=/tmp/x", "--json", "pane"][..],
        &["--state-dir=/tmp/x", "pane", "--json"][..],
        &["--state-dir", "/tmp/x", "--json", "pane"][..],
    ] {
        let v = usage_json(a);
        assert_eq!(v.get("cmd"), Some(&serde_json::json!("pane")), "{a:?}");
    }
}

#[test]
fn recovers_globals_after_error() {
    let (opts, cmd) = super::args_probe::recover(&argv(&[
        "--state-dir=/tmp/x",
        "pane",
        "--bogus",
        "--quiet",
        "--json",
    ]));
    assert_eq!(cmd, "pane");
    assert!(opts.json && opts.quiet);
    assert_eq!(opts.state_dir, Some("/tmp/x".into()));
}

#[test]
fn json_after_invalid_arg_is_usage_json() {
    for c in ["pane", "start"] {
        let v = usage_json(&[c, "--bogus", "--json"]);
        assert_eq!(v.get("cmd"), Some(&serde_json::json!(c)));
    }
}

#[test]
fn gate_record_and_missing_name_are_usage() {
    for a in [
        &["gate", "record", "abc1234", "lint"][..],
        &["gate", "check", "abc1234"][..],
    ] {
        assert_eq!(done(a).code, 2, "{a:?}");
    }
}

/// Parses a request from argv, expecting one.
fn request(a: &[&str]) -> Request {
    let Parsed::Request(inv) = parse(argv(a)) else {
        unreachable!("expected Request")
    };
    inv.request
}

/// Whether a run request inherits the output.
fn inherits(r: &Request) -> bool {
    matches!(r, Request::Run(run) if run.inherit_output)
}

#[test]
fn text_run_inherits_output() {
    let r = inherit_run_output(request(&["run", "t", "--", "ls"]), &GlobalOpts::default());
    assert!(inherits(&r));
}

#[test]
fn json_run_keeps_output_in_the_log() {
    let opts = GlobalOpts {
        json: true,
        ..GlobalOpts::default()
    };
    let r = inherit_run_output(request(&["run", "t", "--", "ls"]), &opts);
    assert!(!inherits(&r));
}

#[test]
fn non_run_requests_are_untouched() {
    let before = request(&["env"]);
    let after = inherit_run_output(request(&["env"]), &GlobalOpts::default());
    assert_eq!(format!("{before:?}"), format!("{after:?}"));
}
