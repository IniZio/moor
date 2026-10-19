//! Tests for `show-session`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::request::ShowSessionReq;
use moor_core::{ErrorKind, Request, SessionId};

use super::run;
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

fn req(session: i64) -> ShowSessionReq {
    ShowSessionReq {
        session: SessionId::new(session),
    }
}

#[test]
fn a_host_session_shows_its_workdir_agent_session_and_state() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let id = cx
        .store()
        .unwrap()
        .open_host_session("/home/me/proj", Some("claude"), Some("abc"))
        .unwrap();
    let v = run(&cx, &req(id)).unwrap();
    assert_eq!(v.session.get(), id);
    assert_eq!(v.box_name, None);
    assert_eq!(v.workdir.as_deref(), Some("/home/me/proj"));
    assert_eq!(v.agent.as_deref(), Some("claude"));
    assert_eq!(v.agent_session_id.as_deref(), Some("abc"));
    assert_eq!(v.state, "working");
    assert!(v.children.is_empty());
}

#[test]
fn children_list_with_their_box_and_state() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let store = cx.store().unwrap();
    let parent = store
        .open_host_session("/home/me/proj", None, None)
        .unwrap();
    let child = store.open_host_session("/home/me/c", None, None).unwrap();
    let db = rusqlite::Connection::open(dir.path().join("moor.db")).unwrap();
    db.execute(
        "UPDATE sessions SET parent_session = ?1 WHERE id = ?2",
        [parent, child],
    )
    .unwrap();
    let v = run(&cx, &req(parent)).unwrap();
    assert_eq!(v.children.len(), 1);
    let c = v.children.first().unwrap();
    assert_eq!(c.session.get(), child);
    assert_eq!(c.box_name, None);
    assert_eq!(c.state, "working");
    assert_eq!(
        run(&cx, &req(child)).unwrap().parent,
        Some(SessionId::new(parent))
    );
}

#[test]
fn an_unknown_session_fails() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let err = run(&cx, &req(999)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert_eq!(err.error.to_string(), "no Session 999");
}

#[test]
fn dispatch_returns_workdir_and_children() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let id = cx
        .store()
        .unwrap()
        .open_host_session("/home/me/proj", None, None)
        .unwrap();
    let v = dispatch(&cx, Request::ShowSession(req(id))).unwrap();
    assert_eq!(v.get("workdir").unwrap(), "/home/me/proj");
    assert!(v.get("children").unwrap().is_array());
}
