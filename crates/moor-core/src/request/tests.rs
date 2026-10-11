//! Tests for the request model.

use super::*;
use serde_json::json;

/// Variants carrying a task or arguments.
fn with_args() -> Vec<Request> {
    let t = || TaskReq { task: "a".into() };
    vec![
        Request::Start(StartReq {
            name: "a".into(),
            goal: Some("g".into()),
            issue: None,
            worktree: None,
            base: None,
            model: None,
            no_agent: true,
        }),
        Request::Run(RunReq {
            task: "a".into(),
            no_sync: false,
            record: None,
            argv: vec!["ls".into()],
        }),
        Request::Wait(WaitReq {
            task: "a".into(),
            wait_for: Some(WaitFor::Idle),
            timeout_secs: Some(3),
        }),
        Request::Steer(SteerReq {
            task: "a".into(),
            text: Some("x".into()),
            menu: None,
        }),
        Request::Report(ReportReq {
            task: "a".into(),
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
            task: "a".into(),
            force: true,
        }),
        Request::Remove(RemoveReq {
            task: "a".into(),
            discard: false,
        }),
        Request::Cp(CpReq {
            task: "a".into(),
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
            action: GateAction::Record,
            sha: "abcd".into(),
            name: "n".into(),
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
            task: "a".into(),
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
            task: "a".into(),
            wait_for: Some(WaitFor::Done),
            timeout_secs: None
        })
    );
    let r: Request = serde_json::from_str(r#"{"cmd":"run","task":"a"}"#).unwrap();
    assert_eq!(
        r,
        Request::Run(RunReq {
            task: "a".into(),
            no_sync: false,
            record: None,
            argv: vec![]
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
    let v = json!({"cmd":"run","task":"a","no_sync":false,"argv":["ls","-la"]});
    let r: Request = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(
        r,
        Request::Run(RunReq {
            task: "a".into(),
            no_sync: false,
            record: None,
            argv: vec!["ls".into(), "-la".into()],
        })
    );
    assert_eq!(serde_json::to_value(&r).unwrap(), v);
}
