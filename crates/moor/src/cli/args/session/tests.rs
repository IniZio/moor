//! Tests for the `session set` and `session end` arguments.

use std::ffi::OsString;

use moor_core::request::{EndHostSessionReq, UpdateHostSessionReq};
use moor_core::{Request, SessionId};

use crate::cli::args::{parse, Action};

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
