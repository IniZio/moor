//! Tests for the request model.

use super::*;
use serde_json::json;

/// Valid task name.
fn n(s: &str) -> TaskName {
    TaskName::parse(s).expect("valid name")
}

/// Variants carrying a task or arguments.
#[allow(clippy::too_many_lines, reason = "one sample per request variant")]
fn with_args() -> Vec<Request> {
    let t = || TaskReq { task: n("a") };
    vec![
        Request::Start(StartReq {
            name: n("a"),
            goal: Some("g".into()),
            issue: None,
            worktree: None,
            base: None,
            model: None,
            no_agent: true,
            resume: false,
        }),
        Request::Run(RunReq {
            task: n("a"),
            no_sync: false,
            sync: false,
            record: None,
            argv: vec!["ls".into()],
            inherit_output: false,
        }),
        Request::Wait(WaitReq {
            task: n("a"),
            wait_for: Some(WaitFor::Idle),
            timeout_secs: Some(3),
        }),
        Request::Steer(SteerReq {
            task: n("a"),
            text: Some("x".into()),
            menu: None,
        }),
        Request::Report(ReportReq {
            task: n("a"),
            kind: "k".into(),
            title: None,
            file: None,
        }),
        Request::Show(ShowReq {
            task: None,
            events: Some(2),
            agent: false,
            reports: true,
            pane: false,
        }),
        Request::Close(CloseReq {
            task: n("a"),
            force: true,
        }),
        Request::Remove(RemoveReq {
            task: n("a"),
            discard: false,
        }),
        Request::Cp(CpReq {
            task: n("a"),
            src: "s".into(),
            dest: "d".into(),
        }),
        Request::Open(t()),
        Request::Shell(t()),
        Request::Sleep(t()),
        Request::Wake(t()),
    ]
}

/// One value of every variant.
fn all() -> Vec<Request> {
    let mut v = with_args();
    v.extend([
        Request::Gate(GateReq {
            sha: "abcd".into(),
            name: "n".into(),
            command: None,
        }),
        Request::Env(EnvReq { repo: None }),
        Request::GithubSetup {},
        Request::GithubLogin {},
        Request::ProxyCheck {},
        Request::Version {},
    ]);
    v
}

#[test]
fn cmd_equals_serde_tag_and_roundtrips() {
    let reqs = all();
    assert_eq!(reqs.len(), 19);
    for r in reqs {
        let text = serde_json::to_string(&r).unwrap();
        assert!(
            text.starts_with(&format!(r#"{{"cmd":"{}""#, r.cmd())),
            "{text}"
        );
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v.get("cmd"), Some(&json!(r.cmd())));
        let back: Request = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
    }
}

#[test]
fn close_roundtrips_from_json() {
    let r: Request = serde_json::from_str(r#"{"cmd":"close","task":"a","force":true}"#).unwrap();
    assert_eq!(
        r,
        Request::Close(CloseReq {
            task: n("a"),
            force: true
        })
    );
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        json!({"cmd":"close","task":"a","force":true})
    );
}

#[test]
fn unknown_field_rejected() {
    let r = serde_json::from_str::<Request>(r#"{"cmd":"close","task":"a","bogus":1}"#);
    assert!(r.is_err());
    let r = serde_json::from_str::<Request>(r#"{"cmd":"nope"}"#);
    assert!(r.is_err());
}

#[test]
fn unit_variant_accepted() {
    let r: Request = serde_json::from_str(r#"{"cmd":"version"}"#).unwrap();
    assert_eq!(r, Request::Version {});
}

#[test]
fn unit_variants_reject_unknown_fields() {
    for cmd in ["github-setup", "github-login", "proxy-check", "version"] {
        let ok = format!(r#"{{"cmd":"{cmd}"}}"#);
        assert!(serde_json::from_str::<Request>(&ok).is_ok(), "{ok}");
        let bad = format!(r#"{{"cmd":"{cmd}","bogus":1}}"#);
        assert!(serde_json::from_str::<Request>(&bad).is_err(), "{bad}");
    }
}

#[test]
fn defaults_and_renames() {
    let r: Request = serde_json::from_str(r#"{"cmd":"wait","task":"a","for":"done"}"#).unwrap();
    assert_eq!(
        r,
        Request::Wait(WaitReq {
            task: n("a"),
            wait_for: Some(WaitFor::Done),
            timeout_secs: None
        })
    );
    let r: Request = serde_json::from_str(r#"{"cmd":"run","task":"a"}"#).unwrap();
    assert_eq!(
        r,
        Request::Run(RunReq {
            task: n("a"),
            no_sync: false,
            sync: false,
            record: None,
            argv: vec![],
            inherit_output: false,
        })
    );
}

#[test]
fn none_fields_omitted() {
    let v = serde_json::to_value(Request::Env(EnvReq { repo: None })).unwrap();
    assert_eq!(v, json!({"cmd":"env"}));
}

#[test]
fn run_argv_roundtrips() {
    let v = json!({"cmd":"run","task":"a","no_sync":false,"sync":false,"argv":["ls","-la"]});
    let r: Request = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(
        r,
        Request::Run(RunReq {
            task: n("a"),
            no_sync: false,
            sync: false,
            record: None,
            argv: vec!["ls".into(), "-la".into()],
            inherit_output: false,
        })
    );
    assert_eq!(serde_json::to_value(&r).unwrap(), v);
}

#[test]
fn run_inherit_output_is_not_settable_from_json() {
    let v = json!({"cmd":"run","task":"a","inherit_output":true});
    assert!(serde_json::from_value::<Request>(v).is_err());
    let r = RunReq {
        task: n("a"),
        no_sync: false,
        sync: false,
        record: None,
        argv: vec![],
        inherit_output: true,
    };
    let v = serde_json::to_value(Request::Run(r)).unwrap();
    assert!(v.get("inherit_output").is_none());
}

#[test]
fn gate_command_is_optional_on_the_wire() {
    let bare = Request::Gate(GateReq {
        sha: "abcd".into(),
        name: "lint".into(),
        command: None,
    });
    let text = serde_json::to_string(&bare).unwrap();
    assert_eq!(text, r#"{"cmd":"gate","sha":"abcd","name":"lint"}"#);
    assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), bare);

    let with = Request::Gate(GateReq {
        sha: "abcd".into(),
        name: "lint".into(),
        command: Some("make lint".into()),
    });
    let text = serde_json::to_string(&with).unwrap();
    assert!(text.contains(r#""command":"make lint""#), "{text}");
    assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), with);
}

#[test]
fn run_sync_flag_parses_and_defaults_off() {
    let with: Request =
        serde_json::from_str(r#"{"cmd":"run","task":"a","sync":true,"argv":["ls"]}"#).unwrap();
    assert_eq!(
        with,
        Request::Run(RunReq {
            task: n("a"),
            no_sync: false,
            sync: true,
            record: None,
            argv: vec!["ls".into()],
            inherit_output: false,
        })
    );
    let without: Request = serde_json::from_str(r#"{"cmd":"run","task":"a"}"#).unwrap();
    let want = Request::Run(RunReq {
        task: n("a"),
        no_sync: false,
        sync: false,
        record: None,
        argv: vec![],
        inherit_output: false,
    });
    assert_eq!(without, want);
}
