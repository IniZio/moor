//! Tests for the `session` arguments and the `--parent` flag.

use std::ffi::OsString;

use moor_core::request::{
    EndHostSessionReq, OpenHostSessionReq, ShowSessionReq, UpdateHostSessionReq,
};
use moor_core::{Request, SessionId};

use crate::cli::args::{parse_ignoring_env as parse, Action};

/// Parse `moor <args>`.
fn run(args: &[&str]) -> Result<Action, clap::Error> {
    let mut v: Vec<OsString> = vec!["moor".into()];
    v.extend(args.iter().map(OsString::from));
    parse(v).map(|(_, a)| a)
}

/// Parse and expect a plain request.
fn req(args: &[&str]) -> Request {
    match run(args).expect("parses") {
        Action::Request(r) => r,
        _ => Request::Version {},
    }
}

#[test]
fn session_set_takes_agent_session_id() {
    let r = req(&["session", "set", "4", "--agent-session-id", "abc"]);
    assert_eq!(
        r,
        Request::UpdateHostSession(UpdateHostSessionReq {
            session: SessionId::new(4),
            agent: None,
            agent_session_id: Some("abc".into()),
        })
    );
}

#[test]
fn session_set_needs_a_field() {
    assert!(run(&["session", "set", "4"]).is_err());
}

#[test]
fn session_end_takes_id() {
    let r = req(&["session", "end", "4"]);
    assert_eq!(
        r,
        Request::EndHostSession(EndHostSessionReq {
            session: SessionId::new(4),
        })
    );
}

#[test]
fn session_open_parent_lands_in_request() {
    let r = req(&["session", "open", "t", "--seed", "x", "--parent", "7"]);
    assert!(
        matches!(&r, Request::OpenSession(o) if o.parent == Some(SessionId::new(7))),
        "{r:?}"
    );
}

#[test]
fn session_open_host_parent_lands_in_request() {
    let r = req(&[
        "session",
        "open",
        "--host",
        "--workdir",
        "/w",
        "--parent",
        "4",
    ]);
    assert_eq!(
        r,
        Request::OpenHostSession(OpenHostSessionReq {
            workdir: "/w".into(),
            agent: None,
            agent_session_id: None,
            parent: Some(SessionId::new(4)),
        })
    );
}

#[test]
fn start_parent_lands_in_request() {
    let r = req(&["start", "t", "--parent", "7"]);
    assert!(
        matches!(&r, Request::Start(s) if s.parent == Some(SessionId::new(7))),
        "{r:?}"
    );
}

#[test]
fn parent_defaults_from_moor_session_env() {
    use clap::CommandFactory;
    let cmd = crate::cli::args::Cli::command();
    let env_of = |c: &clap::Command| {
        c.get_arguments()
            .find(|a| a.get_id() == "parent")
            .and_then(|a| a.get_env().map(std::ffi::OsStr::to_os_string))
    };
    let start = cmd.find_subcommand("start").expect("start");
    let open = cmd
        .find_subcommand("session")
        .and_then(|s| s.find_subcommand("open"))
        .expect("session open");
    assert_eq!(env_of(start), Some("MOOR_SESSION".into()));
    assert_eq!(env_of(open), Some("MOOR_SESSION".into()));
}

#[test]
fn show_session_maps_to_show_session() {
    assert_eq!(
        req(&["show", "--session", "5"]),
        Request::ShowSession(ShowSessionReq {
            session: SessionId::new(5)
        })
    );
}

#[test]
fn show_session_refuses_task_and_flags() {
    assert!(run(&["show", "--session", "5", "sometask"]).is_err());
    assert!(run(&["show", "--session", "5", "--pane"]).is_err());
}

#[test]
fn plain_show_stays_show() {
    assert!(matches!(req(&["show"]), Request::Show(_)));
}
