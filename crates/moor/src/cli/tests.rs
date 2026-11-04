//! Tests for the cli module.

use std::ffi::OsString;

use moor_core::{Request, Response};

use super::{inherit_run_output, parse, parse_call, render, GlobalOpts, Parsed};

/// Builds argv with a program name.
pub(super) fn argv(a: &[&str]) -> Vec<OsString> {
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
fn proxy_serve_parses_box_id() {
    assert!(matches!(
        parse(argv(&["proxy", "serve", "7"])),
        Parsed::ProxyServe { box_id, .. } if box_id == moor_core::BoxId::new(7)
    ));
}

#[test]
fn proxy_serve_without_box_id_is_usage() {
    assert_eq!(done(&["proxy", "serve"]).code, 2);
}

#[test]
fn proxy_serve_non_numeric_box_id_is_usage() {
    assert_eq!(done(&["proxy", "serve", "abc"]).code, 2);
}

#[test]
fn store_migrate_parses_with_the_state_dir() {
    assert!(matches!(
        parse(argv(&["--state-dir", "/tmp/s", "store", "migrate"])),
        Parsed::StoreMigrate { opts } if opts.state_dir.as_deref() == Some(std::path::Path::new("/tmp/s"))
    ));
}

#[test]
fn controller_serve_parses_config() {
    assert!(matches!(
        parse(argv(&["controller", "serve", "--config", "/etc/c.yaml"])),
        Parsed::ControllerServe { config, .. } if config == std::path::Path::new("/etc/c.yaml")
    ));
}

#[test]
fn controller_serve_without_config_is_usage() {
    assert_eq!(done(&["controller", "serve"]).code, 2);
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
fn parse_call_refuses_removed_commands_as_unknown() {
    for cmd in [
        "report",
        "remove",
        "shell",
        "github-setup",
        "github-login",
        "proxy-check",
    ] {
        let o = parse_call(&format!(r#"{{"cmd":"{cmd}"}}"#)).expect_err(cmd);
        let v: serde_json::Value = serde_json::from_str(&o.stdout).expect("json");
        assert_eq!(v.get("class"), Some(&serde_json::json!("usage")), "{cmd}");
        let message = v
            .pointer("/error/message")
            .and_then(serde_json::Value::as_str)
            .expect("message");
        assert!(
            message.contains(&format!("unknown variant `{cmd}`")),
            "{message}"
        );
        assert_eq!(o.code, 2, "{cmd}");
    }
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
fn render_json_open_session_prints_value_fields() {
    let v = serde_json::json!({"box": "t1", "session": 9, "ordinal": 3, "pane": "p4"});
    let opts = GlobalOpts {
        json: true,
        ..GlobalOpts::default()
    };
    let o = render(&Response::ok("open-session", Some(v)), &opts);
    let got: serde_json::Value = serde_json::from_str(&o.stdout).expect("json");
    assert_eq!(got.pointer("/value/session"), Some(&serde_json::json!(9)));
    assert_eq!(got.pointer("/value/pane"), Some(&serde_json::json!("p4")));
}

#[test]
fn render_json_end_session_prints_value_fields() {
    let v = serde_json::json!({"box": "t1", "session": 7, "ordinal": 2, "box_kept": true});
    let opts = GlobalOpts {
        json: true,
        ..GlobalOpts::default()
    };
    let o = render(&Response::ok("end-session", Some(v)), &opts);
    let got: serde_json::Value = serde_json::from_str(&o.stdout).expect("json");
    assert_eq!(got.pointer("/value/ordinal"), Some(&serde_json::json!(2)));
    assert_eq!(
        got.pointer("/value/box_kept"),
        Some(&serde_json::json!(true))
    );
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

/// The request a successful parse of `a` runs.
fn parsed_request(a: &[&str]) -> Request {
    let Parsed::Request(inv) = parse(argv(a)) else {
        unreachable!("expected a request for {a:?}")
    };
    inv.request
}

#[test]
fn user_verbs_assert_no_identity_so_the_socket_supplies_local() {
    for (args, cmd) in [
        (&["whoami"][..], "whoami"),
        (&["user", "enroll"][..], "enroll"),
        (&["user", "secret", "rm", "claude"][..], "secret-delete"),
        (&["user", "secret", "ls"][..], "secret-names"),
        (
            &["user", "profile", "set", "https://github.com/alice/agent"][..],
            "profile-set",
        ),
        (&["user", "profile", "show"][..], "profile-show"),
        (&["user", "profile", "rm"][..], "profile-remove"),
    ] {
        let req = parsed_request(args);
        assert_eq!(req.cmd(), cmd);
        let (asserted, _) = req.into_user().expect("a user request");
        assert_eq!(asserted, None, "{args:?}");
    }
}

#[test]
fn secret_set_takes_only_a_name_and_reads_the_value_from_stdin() {
    let Parsed::Stdin { then, .. } = parse(argv(&["user", "secret", "set", "claude"])) else {
        unreachable!("expected a stdin request")
    };
    let o = done(&["--json", "user", "secret", "set", "claude", "the-value"]);
    assert_eq!(o.code, 2, "{}", o.stdout);
    let req = then.request(b"tok\r\n\n".to_vec());
    let Ok((None, moor_core::request::UserCmd::SecretSet { value, .. })) = req.into_user() else {
        unreachable!("expected secret-set")
    };
    assert_eq!(value.secret().expose(), b"tok");
}

#[test]
fn profile_set_takes_a_github_url_with_an_optional_ref() {
    let req = parsed_request(&[
        "user",
        "profile",
        "set",
        "https://github.com/alice/agent.git#v2",
    ]);
    let Ok((None, moor_core::request::UserCmd::ProfileSet { source })) = req.into_user() else {
        unreachable!("expected profile-set")
    };
    assert_eq!(source.to_string(), "https://github.com/alice/agent#v2");
    let o = done(&["user", "profile", "set", "https://gitlab.com/alice/agent"]);
    assert_eq!(o.code, 2, "{}", o.stdout);
}

#[test]
fn webhook_add_ls_and_rm_build_their_requests() {
    use moor_core::request::UserCmd;
    let req = parsed_request(&[
        "user",
        "webhook",
        "add",
        "https://x.example/h",
        "--event",
        "idled",
        "--event",
        "closed",
    ]);
    let Ok((
        None,
        UserCmd::WebhookAdd {
            url,
            events,
            header,
        },
    )) = req.into_user()
    else {
        unreachable!("expected webhook-add")
    };
    assert_eq!(url, "https://x.example/h");
    assert_eq!(events, Some(vec!["idled".to_owned(), "closed".to_owned()]));
    assert_eq!(header, None);
    let bare = parsed_request(&["user", "webhook", "add", "https://x.example/h"]);
    assert!(matches!(
        bare.into_user(),
        Ok((None, UserCmd::WebhookAdd { events: None, .. }))
    ));
    let ls = parsed_request(&["user", "webhook", "ls"]);
    assert!(matches!(ls.into_user(), Ok((None, UserCmd::WebhookList))));
    let rm = parsed_request(&["user", "webhook", "rm", "7"]);
    assert!(matches!(
        rm.into_user(),
        Ok((None, UserCmd::WebhookRemove { id: 7 }))
    ));
}

#[test]
fn webhook_header_setup_and_finish_keep_the_header_value_out_of_argv() {
    use moor_core::request::UserCmd;
    let req = parsed_request(&[
        "user",
        "webhook",
        "add",
        "https://x.example/h",
        "--header",
        "Authorization",
    ]);
    assert!(matches!(
        req.into_user(),
        Ok((None, UserCmd::WebhookAdd { header: Some(h), .. })) if h == "Authorization"
    ));
    let setup = parsed_request(&["user", "webhook", "setup", "3"]);
    assert!(matches!(
        setup.into_user(),
        Ok((None, UserCmd::WebhookSetupShow { id: 3 }))
    ));
    let o = done(&["--json", "user", "webhook", "finish", "3", "the-token"]);
    assert_eq!(o.code, 2, "{}", o.stdout);
    let Parsed::Stdin { then, .. } = parse(argv(&["user", "webhook", "finish", "3"])) else {
        unreachable!("expected a stdin request")
    };
    let Ok((
        None,
        UserCmd::WebhookSetupFinish {
            id: 3,
            header_value: Some(v),
        },
    )) = then.clone().request(b"Bearer t\n".to_vec()).into_user()
    else {
        unreachable!("expected webhook-setup-finish with a value")
    };
    assert_eq!(v.secret().expose(), b"Bearer t");
    assert!(matches!(
        then.request(b"\n".to_vec()).into_user(),
        Ok((
            None,
            UserCmd::WebhookSetupFinish {
                id: 3,
                header_value: None
            }
        ))
    ));
}

#[test]
fn inspect_parses_its_options() {
    let name = moor_core::BoxName::parse_allow_bare("app/x").expect("name");
    assert_eq!(
        request(&["inspect", "app/x", "--lines", "10", "--said", "2"]),
        Request::Inspect(moor_core::request::InspectReq {
            target: name.clone().into(),
            lines: Some(10),
            said: Some(2),
        })
    );
    assert_eq!(
        request(&["inspect", "app/x"]),
        Request::Inspect(moor_core::request::InspectReq {
            target: name.into(),
            lines: None,
            said: None,
        })
    );
}

fn inspect_text(value: serde_json::Value) -> String {
    render(
        &Response::ok("inspect", Some(value)),
        &GlobalOpts::default(),
    )
    .stdout
}

#[test]
fn inspect_text_lays_out_header_stuck_said_and_pane() {
    let out = inspect_text(serde_json::json!({
        "box": "app/x", "lifecycle": "ready", "session": "working",
        "agent": {"launched": true, "pane": "live", "state": "working", "source": "hook"},
        "pane": {"tail": "$ cargo test\nrunning\n", "status": "live"},
        "said": [
            {"text": "first", "at": "2026-11-02T10:00:00Z"},
            {"text": "second\nmore", "at": "2026-11-02T10:05:00Z"}
        ],
        "stuck": {"kind": "stalled", "since": "2026-11-02T10:06:00Z",
                  "detail": "no hook report for 10 min while cargo test"}
    }));
    assert_eq!(
        out,
        "app/x · ready · agent working (hook)\n\
         stuck: stalled since 2026-11-02T10:06:00Z: no hook report for 10 min while cargo test\n\
         said:\n  2026-11-02T10:00:00Z  first\n  2026-11-02T10:05:00Z  second\n                        more\n\
         pane (live, last 2 lines):\n$ cargo test\nrunning\n"
    );
}

#[test]
fn inspect_text_with_nothing_else_is_only_the_header() {
    let out = inspect_text(serde_json::json!({
        "box": "app/x", "lifecycle": "asleep", "session": "idle",
        "agent": null, "pane": null, "said": [], "stuck": null
    }));
    assert_eq!(out, "app/x · asleep\n");
}
