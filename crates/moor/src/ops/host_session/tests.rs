//! Tests for the host Session operations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::request::{EndHostSessionReq, OpenHostSessionReq, UpdateHostSessionReq};
use moor_core::{ErrorKind, Request, SessionId};

use super::{end, open, update};
use crate::cx::Cx;
use crate::ops::dispatch;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

fn cx(dir: &Path) -> Cx {
    Cx::fake(
        dir,
        Path::new("/home/me/proj"),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new(PathBuf::from("/s"))),
        Arc::new(ScriptedProc::new()),
    )
}

fn req(workdir: &str) -> OpenHostSessionReq {
    OpenHostSessionReq {
        workdir: workdir.to_owned(),
        agent: Some("claude".to_owned()),
        agent_session_id: None,
    }
}

#[test]
fn a_valid_workdir_registers_a_boxless_working_session() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let v = open(&cx, &req("/home/me/proj")).unwrap();
    assert_eq!(v.workdir, "/home/me/proj");
    assert_eq!(v.state, "working");
    let db = rusqlite::Connection::open(dir.path().join("moor.db")).unwrap();
    let (boxed, workdir): (Option<i64>, String) = db
        .query_row(
            "SELECT box, workdir FROM sessions WHERE id = ?1",
            [v.session.get()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(boxed, None);
    assert_eq!(workdir, "/home/me/proj");
}

#[test]
fn a_relative_or_empty_workdir_is_refused_as_usage() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    for bad in ["proj", ""] {
        let err = open(&cx, &req(bad)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage, "{bad:?}");
    }
}

#[test]
fn dispatch_routes_the_request_and_returns_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let v = dispatch(&cx, Request::OpenHostSession(req("/home/me/proj"))).unwrap();
    assert!(v.get("session").is_some_and(serde_json::Value::is_i64));
}

fn update_req(session: SessionId, id: Option<&str>) -> UpdateHostSessionReq {
    UpdateHostSessionReq {
        session,
        agent: None,
        agent_session_id: id.map(str::to_owned),
    }
}

#[test]
fn update_sets_the_agent_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let id = open(&cx, &req("/home/me/proj")).unwrap().session;
    let v = update(&cx, &update_req(id, Some("abc"))).unwrap();
    assert_eq!(v.state, "working");
    let row = cx.store().unwrap().host_session(id.get()).unwrap().unwrap();
    assert_eq!(row.agent_session_id.as_deref(), Some("abc"));
}

#[test]
fn update_with_no_fields_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let id = open(&cx, &req("/home/me/proj")).unwrap().session;
    let err = update(&cx, &update_req(id, None)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn an_ended_session_refuses_further_updates() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let id = open(&cx, &req("/home/me/proj")).unwrap().session;
    end(&cx, &EndHostSessionReq { session: id }).unwrap();
    let err = update(&cx, &update_req(id, Some("abc"))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
}

#[test]
fn ending_an_unknown_session_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let err = end(
        &cx,
        &EndHostSessionReq {
            session: SessionId::new(9999),
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
}

#[test]
fn dispatch_routes_update_and_end() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let id = open(&cx, &req("/home/me/proj")).unwrap().session;
    let u = dispatch(&cx, Request::UpdateHostSession(update_req(id, Some("abc")))).unwrap();
    assert!(u.get("state").is_some());
    let e = dispatch(
        &cx,
        Request::EndHostSession(EndHostSessionReq { session: id }),
    )
    .unwrap();
    assert!(e.get("state").is_some());
}
