//! Tests for `open-host-session`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::request::OpenHostSessionReq;
use moor_core::{ErrorKind, Request};

use super::open;
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
