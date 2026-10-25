//! Tests for the clap surface.

use std::ffi::OsString;
use std::path::PathBuf;

use moor_core::request::{EndSessionReq, OpenHostSessionReq, OpenSessionReq};
use moor_core::seam::forge::ForgeRepo;
use moor_core::{
    BoxId, BoxName, BoxReq, CloseReq, CpReq, EnvReq, GateReq, Request, RunReq, ShowReq, StartReq,
    SteerReq, WaitFor, WaitReq,
};

use super::{parse_ignoring_env as parse, Action};
use crate::cli::GlobalOpts;

mod events;

/// Parse `moor <args>` into opts and action.
fn run(args: &[&str]) -> Result<(GlobalOpts, Action), clap::Error> {
    let mut v: Vec<OsString> = vec!["moor".into()];
    v.extend(args.iter().map(OsString::from));
    parse(v)
}

/// Parse and expect a plain request.
fn req(args: &[&str]) -> Request {
    let (_, action) = run(args).expect("parses");
    assert!(
        matches!(action, Action::Request(_)),
        "not a request: {args:?}"
    );
    match action {
        Action::Request(r) => r,
        _ => Request::Version {},
    }
}

/// Valid Box name.
fn n(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).expect("valid name")
}

/// Box request helper.
fn box_req(t: &str) -> BoxReq {
    BoxReq { box_name: n(t) }
}

#[test]
fn start_with_all_flags() {
    let r = req(&[
        "start",
        "t1",
        "--goal",
        "g",
        "--issue",
        "I-1",
        "--base",
        "main",
        "--model",
        "haiku",
        "--no-agent",
        "--resume",
    ]);
    assert_eq!(
        r,
        Request::Start(StartReq {
            name: n("t1"),
            goal: Some("g".into()),
            issue: Some("I-1".into()),
            base: Some("main".into()),
            model: Some("haiku".into()),
            no_agent: true,
            hold: false,
            resume: true,
            repo: None,
            parent: None,
            identity: None,
        })
    );
}

#[test]
fn start_hold_flag() {
    assert!(matches!(
        req(&["start", "t", "--hold"]),
        Request::Start(s) if s.hold
    ));
    assert!(matches!(
        req(&["start", "t"]),
        Request::Start(s) if !s.hold
    ));
    assert!(run(&["start", "t", "--hold", "--no-agent"]).is_err());
}

#[test]
fn start_repo_flag() {
    assert!(matches!(
        req(&["start", "t", "--repo", "o/doorlock-app"]),
        Request::Start(s) if s.repo == Some(ForgeRepo::new("o", "doorlock-app"))
    ));
}

#[test]
fn run_with_trailing_command() {
    let r = req(&["run", "t1", "--", "ls", "-la"]);
    assert_eq!(
        r,
        Request::Run(RunReq {
            box_name: n("t1"),
            record: None,
            argv: vec!["ls".into(), "-la".into()],
            inherit_output: false,
        })
    );
    let r = req(&["run", "t1", "--record", "r", "--", "make"]);
    assert_eq!(
        r,
        Request::Run(RunReq {
            box_name: n("t1"),
            record: Some("r".into()),
            argv: vec!["make".into()],
            inherit_output: false,
        })
    );
    assert!(run(&["run", "t1"]).is_err());
}

#[test]
fn run_usage_error_exit_code() {
    let argv = ["moor", "run", "t1", "--record"].map(OsString::from);
    let crate::cli::Parsed::Done(o) = crate::cli::parse(argv.to_vec()) else {
        unreachable!("expected Done")
    };
    // A usage error on `run` maps to the run-not-started code, not 2.
    assert_eq!(o.code, 125);
}

#[test]
fn wait_for_and_timeout() {
    assert_eq!(
        req(&["wait", "t1", "--for", "idle", "--timeout", "5"]),
        Request::Wait(WaitReq {
            box_name: n("t1"),
            wait_for: Some(WaitFor::Idle),
            timeout_secs: Some(5),
        })
    );
    assert_eq!(
        req(&["wait", "t1", "--for", "input"]),
        Request::Wait(WaitReq {
            box_name: n("t1"),
            wait_for: Some(WaitFor::Input),
            timeout_secs: None,
        })
    );
    assert!(run(&["wait", "t1", "--for", "bogus"]).is_err());
}

#[test]
fn end_parses_to_end_session() {
    assert_eq!(
        req(&["end", "t1"]),
        Request::EndSession(EndSessionReq {
            box_name: "t1".into()
        })
    );
    assert!(run(&["end"]).is_err());
}

#[test]
fn session_open_takes_seed_text() {
    assert_eq!(
        req(&["session", "open", "t1", "--seed", "hi"]),
        Request::OpenSession(OpenSessionReq {
            box_name: "t1".into(),
            seed: "hi".into(),
            parent: None,
        })
    );
}

#[test]
fn session_open_host_takes_workdir_agent_and_session() {
    assert_eq!(
        req(&[
            "session",
            "open",
            "--host",
            "--workdir",
            "/w",
            "--agent",
            "claude",
            "--agent-session-id",
            "abc"
        ]),
        Request::OpenHostSession(OpenHostSessionReq {
            workdir: Some("/w".into()),
            agent: Some("claude".into()),
            agent_session_id: Some("abc".into()),
            parent: None,
            key: None,
        })
    );
}

#[test]
fn session_open_host_refuses_box_seed_and_missing_workdir() {
    assert!(run(&["session", "open", "--host", "t", "--workdir", "/w"]).is_err());
    assert!(run(&[
        "session",
        "open",
        "--host",
        "--workdir",
        "/w",
        "--seed",
        "x"
    ])
    .is_err());
    assert!(run(&["session", "open", "--host"]).is_err());
    assert!(run(&["session", "open", "t1", "--workdir", "/w"]).is_err());
}

#[test]
fn session_open_host_makes_relative_workdir_absolute() {
    let r = req(&["session", "open", "--host", "--workdir", "w"]);
    assert!(matches!(
        r,
        Request::OpenHostSession(ref h)
            if h.workdir.as_deref().is_some_and(|w| std::path::Path::new(w).is_absolute() && w.ends_with('w'))
    ));
}

#[test]
fn session_open_reads_seed_file_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("seed.txt");
    std::fs::write(&path, "line one\n  line two\n\n").expect("write");
    let p = path.to_str().expect("utf8");
    assert_eq!(
        req(&["session", "open", "t1", "--seed-file", p]),
        Request::OpenSession(OpenSessionReq {
            box_name: "t1".into(),
            seed: "line one\n  line two\n\n".into(),
            parent: None,
        })
    );
    assert!(run(&["session", "open", "t1", "--seed-file", "/no/such/file"]).is_err());
}

#[test]
fn session_open_needs_exactly_one_seed_source() {
    assert!(run(&["session", "open", "t1"]).is_err());
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("seed.txt");
    std::fs::write(&path, "x").expect("write");
    let p = path.to_str().expect("utf8");
    assert!(run(&["session", "open", "t1", "--seed", "a", "--seed-file", p]).is_err());
}

#[test]
fn steer_text_or_menu() {
    assert_eq!(
        req(&["steer", "t1", "go on"]),
        Request::Steer(SteerReq {
            target: n("t1").into(),
            text: Some("go on".into()),
            menu: None,
            interrupt: false,
        })
    );
    assert_eq!(
        req(&["steer", "t1", "--menu", "2"]),
        Request::Steer(SteerReq {
            target: n("t1").into(),
            text: None,
            menu: Some(2),
            interrupt: false,
        })
    );
    assert_eq!(
        req(&["steer", "t1", "--interrupt"]),
        Request::Steer(SteerReq {
            target: n("t1").into(),
            text: None,
            menu: None,
            interrupt: true
        })
    );
    assert!(run(&["steer", "t1"]).is_err());
    assert!(run(&["steer", "t1", "x", "--menu", "1"]).is_err());
    assert!(run(&["steer", "t1", "x", "--interrupt"]).is_err());
    assert!(run(&["steer", "t1", "--menu", "1", "--interrupt"]).is_err());
}

#[test]
fn close_cp() {
    assert_eq!(
        req(&["close", "t1", "--force"]),
        Request::Close(CloseReq {
            target: n("t1").into(),
            force: true,
        })
    );
    assert_eq!(
        req(&["close", "demo"]),
        Request::Close(CloseReq {
            target: n("demo").into(),
            force: false,
        })
    );
    assert_eq!(
        req(&["cp", "t1", "a.txt", "BOX:b.txt"]),
        Request::Cp(CpReq {
            box_name: n("t1"),
            src: "a.txt".into(),
            dest: "BOX:b.txt".into()
        })
    );
}

#[test]
fn show_and_list_alias() {
    let want = Request::Show(ShowReq {
        target: Some(n("t1").into()),
        events: Some(3),
        agent: true,
        reports: true,
        artifacts: false,
        pane: true,
    });
    let a = ["t1", "--events", "3", "--agent", "--reports", "--pane"];
    let show: Vec<&str> = std::iter::once("show").chain(a).collect();
    let list: Vec<&str> = std::iter::once("list").chain(a).collect();
    assert_eq!(req(&show), want);
    assert_eq!(req(&list), want);
    assert_eq!(
        req(&["show"]),
        Request::Show(ShowReq {
            target: None,
            events: None,
            agent: false,
            reports: false,
            artifacts: false,
            pane: false
        })
    );
}

#[test]
fn box_verbs() {
    assert_eq!(req(&["open", "t1"]), Request::Open(box_req("t1")));
    assert_eq!(req(&["sleep", "t1"]), Request::Sleep(box_req("t1")));
    assert_eq!(req(&["wake", "t1"]), Request::Wake(box_req("t1")));
}

#[test]
fn gate_check() {
    assert_eq!(
        req(&["gate", "check", "abc1234", "lint", "--command", "make lint"]),
        Request::Gate(GateReq {
            sha: "abc1234".into(),
            name: "lint".into(),
            command: Some("make lint".into())
        })
    );
    assert_eq!(
        req(&["gate", "check", "abc1234", "lint"]),
        Request::Gate(GateReq {
            sha: "abc1234".into(),
            name: "lint".into(),
            command: None
        })
    );
    assert!(run(&["gate", "check", "abc1234"]).is_err());
    assert!(run(&["gate", "record", "abc1234", "lint"]).is_err());
}

#[test]
fn admin_verbs() {
    assert_eq!(
        req(&["env"]),
        Request::Env(EnvReq {
            repo: None,
            base: None
        })
    );
    assert_eq!(
        req(&["env", "o/web-app", "--base", "v1"]),
        Request::Env(EnvReq {
            repo: Some(ForgeRepo::new("o", "web-app")),
            base: Some("v1".into())
        })
    );
    assert_eq!(req(&["version"]), Request::Version {});
}

#[test]
fn special_actions() {
    assert!(matches!(
        run(&["proxy", "serve", "7"]),
        Ok((_, Action::ProxyServe(t))) if t == BoxId::new(7)
    ));
    assert!(matches!(run(&["call"]), Ok((_, Action::CallStdin))));
}

#[test]
fn controller_serve_takes_config() {
    assert!(matches!(
        run(&["controller", "serve", "--config", "/x.yaml"]),
        Ok((_, Action::ControllerServe(p))) if p.as_path() == std::path::Path::new("/x.yaml")
    ));
    assert!(run(&["controller", "serve"]).is_err());
}

#[test]
fn controller_attach_socket_is_optional() {
    assert!(matches!(
        run(&["controller", "attach"]),
        Ok((_, Action::ControllerAttach(None)))
    ));
    assert!(matches!(
        run(&["controller", "attach", "--socket", "/x"]),
        Ok((_, Action::ControllerAttach(Some(p)))) if p.as_path() == std::path::Path::new("/x")
    ));
}

#[test]
fn removed_verbs_are_errors() {
    assert!(run(&["pane", "x"]).is_err());
    assert!(run(&["reports", "x"]).is_err());
}

#[test]
fn version_flag_is_disabled() {
    assert!(run(&["--version"]).is_err());
}

#[test]
fn global_flags_anywhere() {
    let want = GlobalOpts {
        json: true,
        quiet: false,
        state_dir: Some(PathBuf::from("/s")),
    };
    let before = run(&["--json", "--state-dir", "/s", "version"]).map(|(o, _)| o);
    let after = run(&["version", "--json", "--state-dir", "/s"]).map(|(o, _)| o);
    assert_eq!(before.ok(), Some(want.clone()));
    assert_eq!(after.ok(), Some(want));
    let quiet = run(&["version", "--quiet"]).map(|(o, _)| o.quiet);
    assert_eq!(quiet.ok(), Some(true));
}

#[test]
fn invalid_box_name_is_a_clap_error() {
    for args in [
        &["start", "BAD_Name"][..],
        &["close", "A"],
        &["run", "Bad", "ls"],
    ] {
        let err = run(args).expect_err("rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }
}

#[test]
fn valid_box_name_parses() {
    assert_eq!(
        req(&["show", "a-1"]),
        Request::Show(ShowReq {
            target: Some(n("a-1").into()),
            events: None,
            agent: false,
            reports: false,
            artifacts: false,
            pane: false,
        })
    );
}
