//! Tests for the request model.

use super::*;
use serde_json::json;

/// Valid Box name.
fn n(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).expect("valid name")
}

/// Variants carrying a task or arguments.
#[allow(clippy::too_many_lines, reason = "one sample per request variant")]
fn with_args() -> Vec<Request> {
    let t = || TaskReq { task: n("a") };
    vec![
        Request::Sessions(SessionsReq { parent: None }),
        Request::Start(StartReq {
            name: n("a"),
            goal: Some("g".into()),
            issue: None,
            worktree: None,
            base: None,
            model: None,
            no_agent: true,
            hold: false,
            resume: false,
            trust_moor_yaml: false,
            repo: None,
            parent: None,
        }),
        Request::Run(RunReq {
            task: n("a"),
            no_sync: false,
            sync: false,
            record: None,
            argv: vec!["ls".into()],
            trust_moor_yaml: false,
            inherit_output: false,
        }),
        Request::Wait(WaitReq {
            task: n("a"),
            wait_for: Some(WaitFor::Idle),
            timeout_secs: Some(3),
        }),
        Request::WaitEvents(WaitEventsReq {
            boxes: vec![n("a"), n("b")],
            kinds: Some(vec!["turn_done".into()]),
            mode: WaitMode::All,
            since: Some(7),
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
            artifacts: false,
            pane: false,
        }),
        Request::Close(CloseReq {
            task: n("a"),
            force: true,
            trust_moor_yaml: true,
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
        Request::Connect(t()),
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
        Request::Repos(ReposReq {}),
        Request::Version {},
    ]);
    v
}

#[test]
fn cmd_equals_serde_tag_and_roundtrips() {
    let reqs = all();
    assert_eq!(reqs.len(), 23);
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
            force: true,
            trust_moor_yaml: false
        })
    );
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        json!({"cmd":"close","task":"a","force":true,"trust_moor_yaml":false})
    );
}

#[test]
fn close_trust_moor_yaml_defaults_false_and_roundtrips() {
    let r: Request = serde_json::from_str(r#"{"cmd":"close","task":"a"}"#).unwrap();
    assert!(matches!(&r, Request::Close(c) if !c.trust_moor_yaml));
    let r: Request =
        serde_json::from_str(r#"{"cmd":"close","task":"a","trust_moor_yaml":true}"#).unwrap();
    assert!(matches!(&r, Request::Close(c) if c.trust_moor_yaml));
    let back: Request = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back, r);
}

#[test]
fn start_repo_roundtrips_and_defaults_none() {
    let r: Request =
        serde_json::from_str(r#"{"cmd":"start","name":"a","repo":"doorlock-app"}"#).unwrap();
    assert!(matches!(&r, Request::Start(s) if s.repo.as_deref() == Some("doorlock-app")));
    let back: Request = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back, r);
    let r: Request = serde_json::from_str(r#"{"cmd":"start","name":"a"}"#).unwrap();
    assert!(matches!(&r, Request::Start(s) if s.repo.is_none()));
    assert!(!serde_json::to_string(&r).unwrap().contains("repo"));
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
    let r: Request = serde_json::from_str(r#"{"cmd":"wait","task":"a","for":"input"}"#).unwrap();
    assert_eq!(
        r,
        Request::Wait(WaitReq {
            task: n("a"),
            wait_for: Some(WaitFor::Input),
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
            trust_moor_yaml: false,
            inherit_output: false,
        })
    );
}

#[test]
fn start_trust_moor_yaml_defaults_false_and_roundtrips() {
    let r: Request = serde_json::from_str(r#"{"cmd":"start","name":"a"}"#).unwrap();
    assert!(matches!(r, Request::Start(ref s) if !s.trust_moor_yaml));

    let r: Request =
        serde_json::from_str(r#"{"cmd":"start","name":"a","trust_moor_yaml":true}"#).unwrap();
    assert!(matches!(r, Request::Start(ref s) if s.trust_moor_yaml));
    let back: Request = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
    assert_eq!(back, r);
}

#[test]
fn none_fields_omitted() {
    let v = serde_json::to_value(Request::Env(EnvReq { repo: None })).unwrap();
    assert_eq!(v, json!({"cmd":"env"}));
}

#[test]
fn run_argv_roundtrips() {
    let v = json!({"cmd":"run","task":"a","no_sync":false,"sync":false,"argv":["ls","-la"],"trust_moor_yaml":false});
    let r: Request = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(
        r,
        Request::Run(RunReq {
            task: n("a"),
            no_sync: false,
            sync: false,
            record: None,
            argv: vec!["ls".into(), "-la".into()],
            trust_moor_yaml: false,
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
        trust_moor_yaml: false,
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
            trust_moor_yaml: false,
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
        trust_moor_yaml: false,
        inherit_output: false,
    });
    assert_eq!(without, want);
}

#[test]
fn run_trust_moor_yaml_defaults_false_and_roundtrips() {
    let r: Request = serde_json::from_str(r#"{"cmd":"run","task":"a"}"#).unwrap();
    assert!(matches!(r, Request::Run(ref s) if !s.trust_moor_yaml));

    let r: Request =
        serde_json::from_str(r#"{"cmd":"run","task":"a","trust_moor_yaml":true}"#).unwrap();
    assert!(matches!(r, Request::Run(ref s) if s.trust_moor_yaml));
    let back: Request = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
    assert_eq!(back, r);
}

#[test]
fn connect_parses_and_roundtrips() {
    let r: Request = serde_json::from_str(r#"{"cmd":"connect","task":"t1"}"#).unwrap();
    assert_eq!(r, Request::Connect(TaskReq { task: n("t1") }));
    let back: Request = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
    assert_eq!(back, r);
}

#[test]
fn end_session_round_trips() {
    let req: Request =
        serde_json::from_value(json!({"cmd": "end-session", "task": "x"})).expect("parse");
    assert_eq!(req, Request::EndSession(EndSessionReq { task: "x".into() }));
    assert_eq!(req.cmd(), "end-session");
    assert_eq!(
        serde_json::to_value(&req).expect("serialize"),
        json!({"cmd": "end-session", "task": "x"})
    );
}

#[test]
fn open_session_round_trips() {
    let req: Request =
        serde_json::from_value(json!({"cmd": "open-session", "task": "x", "seed": "hi"}))
            .expect("parse");
    assert_eq!(
        req,
        Request::OpenSession(OpenSessionReq {
            task: "x".into(),
            seed: "hi".into(),
            parent: None,
        })
    );
    assert_eq!(req.cmd(), "open-session");
    assert_eq!(
        serde_json::to_value(&req).expect("serialize"),
        json!({"cmd": "open-session", "task": "x", "seed": "hi"})
    );
}

#[test]
fn open_host_session_round_trips() {
    let bare: Request =
        serde_json::from_value(json!({"cmd": "open-host-session", "workdir": "/w"}))
            .expect("parse");
    assert_eq!(
        bare,
        Request::OpenHostSession(OpenHostSessionReq {
            workdir: "/w".into(),
            agent: None,
            agent_session_id: None,
            parent: None,
        })
    );
    assert_eq!(bare.cmd(), "open-host-session");
    let full = json!({"cmd": "open-host-session", "workdir": "/w", "agent": "claude", "agent_session_id": "s1"});
    let req: Request = serde_json::from_value(full.clone()).expect("parse");
    assert_eq!(serde_json::to_value(&req).expect("serialize"), full);
    assert!(serde_json::from_value::<Request>(
        json!({"cmd": "open-host-session", "workdir": "/w", "bogus": 1})
    )
    .is_err());
}

#[test]
fn open_host_session_parent_parses_and_round_trips() {
    let json = json!({"cmd": "open-host-session", "workdir": "/w", "parent": 4});
    let req: Request = serde_json::from_value(json.clone()).expect("parse");
    assert!(matches!(&req, Request::OpenHostSession(r) if r.parent == Some(SessionId::new(4))));
    assert_eq!(serde_json::to_value(&req).expect("serialize"), json);
}

#[test]
fn start_without_hold_parses_as_false_and_hold_round_trips() {
    let bare: StartReq = serde_json::from_value(json!({"name": "a"})).expect("parses");
    assert!(!bare.hold);
    let held: StartReq =
        serde_json::from_value(json!({"name": "a", "hold": true})).expect("parses");
    assert!(held.hold);
    let back: StartReq =
        serde_json::from_str(&serde_json::to_string(&held).expect("ser")).expect("de");
    assert_eq!(back, held);
}

#[test]
fn start_parent_parses_and_defaults_none() {
    let with: Request =
        serde_json::from_value(json!({"cmd": "start", "name": "x", "parent": 7})).expect("parses");
    assert!(matches!(with, Request::Start(r) if r.parent == Some(SessionId::new(7))));
    let bare: Request =
        serde_json::from_value(json!({"cmd": "start", "name": "x"})).expect("parses");
    assert!(matches!(bare, Request::Start(r) if r.parent.is_none()));
}

#[test]
fn open_session_parent_parses_and_defaults_none() {
    let with: Request = serde_json::from_value(
        json!({"cmd": "open-session", "task": "t", "seed": "s", "parent": 3}),
    )
    .expect("parses");
    assert!(matches!(with, Request::OpenSession(r) if r.parent == Some(SessionId::new(3))));
    let bare: Request =
        serde_json::from_value(json!({"cmd": "open-session", "task": "t", "seed": "s"}))
            .expect("parses");
    assert!(matches!(bare, Request::OpenSession(r) if r.parent.is_none()));
}

#[test]
fn parent_is_omitted_when_none() {
    let start = StartReq {
        name: n("x"),
        goal: None,
        issue: None,
        worktree: None,
        base: None,
        model: None,
        no_agent: false,
        hold: false,
        resume: false,
        trust_moor_yaml: false,
        repo: None,
        parent: None,
    };
    assert!(!serde_json::to_string(&start)
        .expect("ser")
        .contains("parent"));
    let open = OpenSessionReq {
        task: "t".into(),
        seed: "s".into(),
        parent: None,
    };
    assert!(!serde_json::to_string(&open)
        .expect("ser")
        .contains("parent"));
}

#[test]
fn wait_events_roundtrips_from_json() {
    let r: Request =
        serde_json::from_str(r#"{"cmd":"wait-events","boxes":["a","b"],"mode":"all"}"#).unwrap();
    assert_eq!(
        r,
        Request::WaitEvents(WaitEventsReq {
            boxes: vec![n("a"), n("b")],
            kinds: None,
            mode: WaitMode::All,
            since: None,
            timeout_secs: None,
        })
    );
    assert_eq!(r.cmd(), "wait-events");
    let back: Request = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back, r);
}
