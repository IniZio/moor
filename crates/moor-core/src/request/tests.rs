//! Tests for the request model.

use super::*;
use serde_json::json;

/// Valid Box name.
fn n(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).expect("valid name")
}

/// Variants carrying a box or arguments.
#[allow(clippy::too_many_lines, reason = "one sample per request variant")]
fn with_args() -> Vec<Request> {
    let t = || BoxReq { box_name: n("a") };
    vec![
        Request::Sessions(SessionsReq { parent: None }),
        Request::Start(StartReq {
            name: n("a"),
            prompt: Some("g".into()),
            issue: None,
            base: None,
            model: None,
            provider: None,
            no_agent: true,
            resume: false,
            repo: None,
            parent: None,
            identity: None,
        }),
        Request::Run(RunReq {
            box_name: n("a"),
            record: None,
            argv: vec!["ls".into()],
            inherit_output: false,
        }),
        Request::Wait(WaitReq {
            box_name: n("a"),
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
            target: n("a").into(),
            text: Some("x".into()),
            menu: None,
            interrupt: false,
        }),
        Request::Show(ShowReq {
            target: None,
            events: Some(2),
            agent: false,
            reports: true,
            artifacts: false,
            pane: false,
        }),
        Request::Close(CloseReq {
            target: n("a").into(),
            force: true,
        }),
        Request::Cp(CpReq {
            box_name: n("a"),
            src: "s".into(),
            dest: "d".into(),
        }),
        Request::Open(t()),
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
        Request::Env(EnvReq {
            repo: None,
            base: None,
        }),
        Request::Version {},
    ]);
    v
}

#[test]
fn cmd_equals_serde_tag_and_roundtrips() {
    let reqs = all();
    assert_eq!(reqs.len(), 16);
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
fn an_interrupt_steer_roundtrips() {
    let r = Request::Steer(SteerReq {
        target: n("a").into(),
        text: None,
        menu: None,
        interrupt: true,
    });
    let text = serde_json::to_string(&r).unwrap();
    assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), r);
}

#[test]
fn close_roundtrips_from_json() {
    let r: Request = serde_json::from_str(r#"{"cmd":"close","box":"a","force":true}"#).unwrap();
    assert_eq!(
        r,
        Request::Close(CloseReq {
            target: n("a").into(),
            force: true,
        })
    );
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        json!({"cmd":"close","box":"a","force":true})
    );
}

#[test]
fn start_repo_roundtrips_and_defaults_none() {
    let r: Request =
        serde_json::from_str(r#"{"cmd":"start","name":"a","repo":"o/doorlock-app"}"#).unwrap();
    assert!(matches!(&r, Request::Start(s) if s.repo == Some(ForgeRepo::new("o", "doorlock-app"))));
    let back: Request = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back, r);
    let r: Request = serde_json::from_str(r#"{"cmd":"start","name":"a"}"#).unwrap();
    assert!(matches!(&r, Request::Start(s) if s.repo.is_none()));
    assert!(!serde_json::to_string(&r).unwrap().contains("repo"));
}

#[test]
fn unknown_field_rejected() {
    let r = serde_json::from_str::<Request>(r#"{"cmd":"close","box":"a","bogus":1}"#);
    assert!(r.is_err());
    let r = serde_json::from_str::<Request>(r#"{"cmd":"nope"}"#);
    assert!(r.is_err());
}

#[test]
fn a_start_naming_a_worktree_is_an_unknown_field() {
    let r = serde_json::from_str::<Request>(r#"{"cmd":"start","name":"a","worktree":"/w"}"#);
    let err = r.expect_err("worktree is not a start field").to_string();
    assert!(err.contains("unknown field `worktree`"), "{err}");
}

#[test]
fn unit_variant_accepted() {
    let r: Request = serde_json::from_str(r#"{"cmd":"version"}"#).unwrap();
    assert_eq!(r, Request::Version {});
}

#[test]
fn unit_variants_reject_unknown_fields() {
    let ok = r#"{"cmd":"version"}"#;
    assert!(serde_json::from_str::<Request>(ok).is_ok(), "{ok}");
    let bad = r#"{"cmd":"version","bogus":1}"#;
    assert!(serde_json::from_str::<Request>(bad).is_err(), "{bad}");
}

#[test]
fn defaults_and_renames() {
    let r: Request = serde_json::from_str(r#"{"cmd":"wait","box":"a","for":"done"}"#).unwrap();
    assert_eq!(
        r,
        Request::Wait(WaitReq {
            box_name: n("a"),
            wait_for: Some(WaitFor::Done),
            timeout_secs: None
        })
    );
    let r: Request = serde_json::from_str(r#"{"cmd":"wait","box":"a","for":"input"}"#).unwrap();
    assert_eq!(
        r,
        Request::Wait(WaitReq {
            box_name: n("a"),
            wait_for: Some(WaitFor::Input),
            timeout_secs: None
        })
    );
    let r: Request = serde_json::from_str(r#"{"cmd":"run","box":"a"}"#).unwrap();
    assert_eq!(
        r,
        Request::Run(RunReq {
            box_name: n("a"),
            record: None,
            argv: vec![],
            inherit_output: false,
        })
    );
}

#[test]
fn a_request_naming_a_trust_flag_is_an_unknown_field() {
    for cmd in [
        r#""cmd":"start","name":"a""#,
        r#""cmd":"run","box":"a""#,
        r#""cmd":"close","box":"a""#,
    ] {
        let text = format!(r#"{{{cmd},"trust_moor_yaml":true}}"#);
        let err = serde_json::from_str::<Request>(&text)
            .expect_err(cmd)
            .to_string();
        assert!(err.contains("unknown field `trust_moor_yaml`"), "{err}");
    }
}

#[test]
fn start_repo_is_owner_slash_name() {
    for bad in ["/home/me/app", "moor", "https://github.com/o/n"] {
        let text = format!(r#"{{"cmd":"start","name":"a","repo":"{bad}"}}"#);
        assert!(serde_json::from_str::<Request>(&text).is_err(), "{bad}");
    }
}

#[test]
fn in_and_repos_are_unknown_commands() {
    for text in [
        r#"{"cmd":"repos"}"#,
        r#"{"cmd":"in","dir":"/d","request":{"cmd":"version"}}"#,
    ] {
        assert!(serde_json::from_str::<Request>(text).is_err(), "{text}");
    }
}

#[test]
fn none_fields_omitted() {
    let v = serde_json::to_value(Request::Env(EnvReq {
        repo: None,
        base: None,
    }))
    .unwrap();
    assert_eq!(v, json!({"cmd":"env"}));
}

#[test]
fn run_argv_roundtrips() {
    let v = json!({"cmd":"run","box":"a","argv":["ls","-la"]});
    let r: Request = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(
        r,
        Request::Run(RunReq {
            box_name: n("a"),
            record: None,
            argv: vec!["ls".into(), "-la".into()],
            inherit_output: false,
        })
    );
    assert_eq!(serde_json::to_value(&r).unwrap(), v);
}

#[test]
fn run_inherit_output_is_not_settable_from_json() {
    let v = json!({"cmd":"run","box":"a","inherit_output":true});
    assert!(serde_json::from_value::<Request>(v).is_err());
    let r = RunReq {
        box_name: n("a"),
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
fn a_run_naming_a_sync_is_an_unknown_field() {
    for field in ["sync", "no_sync"] {
        let text = format!(r#"{{"cmd":"run","box":"a","argv":["ls"],"{field}":true}}"#);
        let err = serde_json::from_str::<Request>(&text)
            .expect_err(field)
            .to_string();
        assert!(err.contains(&format!("unknown field `{field}`")), "{err}");
    }
}

#[test]
fn connect_parses_and_roundtrips() {
    let r: Request = serde_json::from_str(r#"{"cmd":"connect","box":"t1"}"#).unwrap();
    assert_eq!(r, Request::Connect(BoxReq { box_name: n("t1") }));
    let back: Request = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
    assert_eq!(back, r);
}

#[test]
fn end_session_round_trips() {
    let req: Request =
        serde_json::from_value(json!({"cmd": "end-session", "box": "x"})).expect("parse");
    assert_eq!(
        req,
        Request::EndSession(EndSessionReq {
            box_name: "x".into()
        })
    );
    assert_eq!(req.cmd(), "end-session");
    assert_eq!(
        serde_json::to_value(&req).expect("serialize"),
        json!({"cmd": "end-session", "box": "x"})
    );
}

#[test]
fn open_session_round_trips() {
    let req: Request =
        serde_json::from_value(json!({"cmd": "open-session", "box": "x", "seed": "hi"}))
            .expect("parse");
    assert_eq!(
        req,
        Request::OpenSession(OpenSessionReq {
            box_name: "x".into(),
            seed: "hi".into(),
            parent: None,
        })
    );
    assert_eq!(req.cmd(), "open-session");
    assert_eq!(
        serde_json::to_value(&req).expect("serialize"),
        json!({"cmd": "open-session", "box": "x", "seed": "hi"})
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
            workdir: Some("/w".into()),
            agent: None,
            agent_session_id: None,
            parent: None,
            key: None,
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
        json!({"cmd": "open-session", "box": "t", "seed": "s", "parent": 3}),
    )
    .expect("parses");
    assert!(matches!(with, Request::OpenSession(r) if r.parent == Some(SessionId::new(3))));
    let bare: Request =
        serde_json::from_value(json!({"cmd": "open-session", "box": "t", "seed": "s"}))
            .expect("parses");
    assert!(matches!(bare, Request::OpenSession(r) if r.parent.is_none()));
}

#[test]
fn parent_is_omitted_when_none() {
    let start = StartReq {
        name: n("x"),
        prompt: None,
        issue: None,
        base: None,
        model: None,
        provider: None,
        no_agent: false,
        resume: false,
        repo: None,
        parent: None,
        identity: None,
    };
    assert!(!serde_json::to_string(&start)
        .expect("ser")
        .contains("parent"));
    let open = OpenSessionReq {
        box_name: "t".into(),
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
