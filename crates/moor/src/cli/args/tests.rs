//! Tests for the clap surface.

use std::ffi::OsString;
use std::path::PathBuf;

use moor_core::{
    CloseReq, CpReq, EnvReq, GateReq, RemoveReq, ReportReq, Request, RunReq, ShowReq, StartReq,
    SteerReq, TaskName, TaskReq, WaitFor, WaitReq,
};

use super::{parse, Action};
use crate::cli::GlobalOpts;

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

/// Valid task name.
fn n(s: &str) -> TaskName {
    TaskName::parse(s).expect("valid name")
}

/// Task request helper.
fn task(t: &str) -> TaskReq {
    TaskReq { task: n(t) }
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
        "--worktree",
        "/w",
        "--base",
        "main",
        "--model",
        "haiku",
        "--no-agent",
    ]);
    assert_eq!(
        r,
        Request::Start(StartReq {
            name: n("t1"),
            goal: Some("g".into()),
            issue: Some("I-1".into()),
            worktree: Some("/w".into()),
            base: Some("main".into()),
            model: Some("haiku".into()),
            no_agent: true,
        })
    );
}

#[test]
fn run_with_trailing_command() {
    let r = req(&["run", "t1", "--", "ls", "-la"]);
    assert_eq!(
        r,
        Request::Run(RunReq {
            task: n("t1"),
            no_sync: false,
            record: None,
            argv: vec!["ls".into(), "-la".into()],
        })
    );
    let r = req(&["run", "t1", "--no-sync", "--record", "r", "--", "make"]);
    assert_eq!(
        r,
        Request::Run(RunReq {
            task: n("t1"),
            no_sync: true,
            record: Some("r".into()),
            argv: vec!["make".into()],
        })
    );
    assert!(run(&["run", "t1"]).is_err());
}

#[test]
fn wait_for_and_timeout() {
    assert_eq!(
        req(&["wait", "t1", "--for", "idle", "--timeout", "5"]),
        Request::Wait(WaitReq {
            task: n("t1"),
            wait_for: Some(WaitFor::Idle),
            timeout_secs: Some(5),
        })
    );
    assert!(run(&["wait", "t1", "--for", "bogus"]).is_err());
}

#[test]
fn steer_text_or_menu() {
    assert_eq!(
        req(&["steer", "t1", "go on"]),
        Request::Steer(SteerReq {
            task: n("t1"),
            text: Some("go on".into()),
            menu: None
        })
    );
    assert_eq!(
        req(&["steer", "t1", "--menu", "2"]),
        Request::Steer(SteerReq {
            task: n("t1"),
            text: None,
            menu: Some(2)
        })
    );
    assert!(run(&["steer", "t1"]).is_err());
    assert!(run(&["steer", "t1", "x", "--menu", "1"]).is_err());
}

#[test]
fn report_close_remove_cp() {
    assert_eq!(
        req(&["report", "t1", "--kind", "k", "--title", "T", "--file", "f"]),
        Request::Report(ReportReq {
            task: n("t1"),
            kind: "k".into(),
            title: Some("T".into()),
            file: Some("f".into()),
        })
    );
    assert_eq!(
        req(&["close", "t1", "--force"]),
        Request::Close(CloseReq {
            task: n("t1"),
            force: true
        })
    );
    assert_eq!(
        req(&["remove", "t1", "--discard"]),
        Request::Remove(RemoveReq {
            task: n("t1"),
            discard: true
        })
    );
    assert_eq!(
        req(&["cp", "t1", "a.txt", "BOX:b.txt"]),
        Request::Cp(CpReq {
            task: n("t1"),
            src: "a.txt".into(),
            dest: "BOX:b.txt".into()
        })
    );
}

#[test]
fn show_and_list_alias() {
    let want = Request::Show(ShowReq {
        task: Some(n("t1")),
        events: Some(3),
        agent: true,
        reports: true,
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
            task: None,
            events: None,
            agent: false,
            reports: false,
            pane: false
        })
    );
}

#[test]
fn task_verbs() {
    assert_eq!(req(&["open", "t1"]), Request::Open(task("t1")));
    assert_eq!(req(&["shell", "t1"]), Request::Shell(task("t1")));
    assert_eq!(req(&["sleep", "t1"]), Request::Sleep(task("t1")));
    assert_eq!(req(&["wake", "t1"]), Request::Wake(task("t1")));
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
    assert_eq!(req(&["env"]), Request::Env(EnvReq { repo: None }));
    assert_eq!(
        req(&["env", "web-app"]),
        Request::Env(EnvReq {
            repo: Some("web-app".into())
        })
    );
    assert_eq!(req(&["github", "setup"]), Request::GithubSetup {});
    assert_eq!(req(&["github", "login"]), Request::GithubLogin {});
    assert_eq!(req(&["proxy", "check"]), Request::ProxyCheck {});
    assert_eq!(req(&["version"]), Request::Version {});
}

#[test]
fn special_actions() {
    assert!(matches!(
        run(&["proxy", "serve"]),
        Ok((_, Action::ProxyServe))
    ));
    assert!(matches!(run(&["call"]), Ok((_, Action::CallStdin))));
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
fn invalid_task_name_is_a_clap_error() {
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
fn valid_task_name_parses() {
    assert_eq!(
        req(&["show", "a-1"]),
        Request::Show(ShowReq {
            task: Some(n("a-1")),
            events: None,
            agent: false,
            reports: false,
            pane: false,
        })
    );
}
