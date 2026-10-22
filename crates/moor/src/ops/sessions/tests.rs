//! Tests for `sessions`.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::request::SessionsReq;
use moor_core::{BoxName, Request, SessionId};
use serde_json::json;

use super::run;
use crate::cx::Cx;
use crate::ops::dispatch;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

fn cx(dir: &Path, driver: &Arc<FakeDriver>) -> Cx {
    Cx::fake(
        dir,
        Path::new("/home/me/proj"),
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
}

fn host(cx: &Cx, workdir: &str) -> i64 {
    cx.store()
        .unwrap()
        .open_host_session(workdir, Some("agent"), None)
        .unwrap()
}

fn db(dir: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.join("moor.db")).unwrap()
}

fn set_parent(dir: &Path, child: i64, parent: i64) {
    db(dir)
        .execute(
            "UPDATE sessions SET parent_session = ?1 WHERE id = ?2",
            [parent, child],
        )
        .unwrap();
}

fn all() -> SessionsReq {
    SessionsReq { parent: None }
}

fn ids(v: &moor_core::view::session::SessionsValue) -> Vec<i64> {
    v.sessions.iter().map(|s| s.session.get()).collect()
}

#[test]
fn ended_sessions_are_left_out_and_a_host_session_has_no_box() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let a = host(&cx, "/a");
    let gone = host(&cx, "/gone");
    let b = host(&cx, "/b");
    db(dir.path())
        .execute("UPDATE sessions SET state = 'ended' WHERE id = ?1", [gone])
        .unwrap();
    let v = run(&cx, &all()).unwrap();
    assert_eq!(ids(&v), vec![a, b]);
    let first = v.sessions.first().unwrap();
    assert_eq!(first.box_name, None);
    assert_eq!(first.workdir.as_deref(), Some("/a"));
    assert_eq!(first.parent, None);
}

#[test]
fn the_parent_filter_returns_direct_live_children_only() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let root = host(&cx, "/r");
    let child = host(&cx, "/c");
    let dead = host(&cx, "/d");
    let grand = host(&cx, "/g");
    set_parent(dir.path(), child, root);
    set_parent(dir.path(), dead, root);
    set_parent(dir.path(), grand, child);
    db(dir.path())
        .execute("UPDATE sessions SET state = 'ended' WHERE id = ?1", [dead])
        .unwrap();
    let v = run(
        &cx,
        &SessionsReq {
            parent: Some(SessionId::new(root)),
        },
    )
    .unwrap();
    assert_eq!(ids(&v), vec![child]);
    assert_eq!(
        v.sessions.first().unwrap().parent,
        Some(SessionId::new(root))
    );
}

#[test]
fn an_unknown_parent_fails() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let err = run(
        &cx,
        &SessionsReq {
            parent: Some(SessionId::new(99)),
        },
    )
    .unwrap_err();
    assert_eq!(err.error.to_string(), "no Session 99");
}

#[test]
fn dispatch_returns_the_wire_shape_and_touches_no_box() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = cx(dir.path(), &driver);
    let a = host(&cx, "/a");
    let b = host(&cx, "/b");
    set_parent(dir.path(), b, a);
    let value = dispatch(&cx, Request::Sessions(all())).unwrap();
    assert_eq!(
        value,
        json!({"sessions": [
            {"session": a, "box": null, "parent": null, "state": "working", "workdir": "/a", "ordinal": 1},
            {"session": b, "box": null, "parent": a, "state": "working", "workdir": "/b", "ordinal": 1},
        ]})
    );
    assert!(driver.calls().is_empty(), "{:?}", driver.calls());
}

fn box_child(cx: &Cx, dir: &Path, name: &str, parent: i64) -> i64 {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: "r".into(),
    };
    cx.store().unwrap().apply(Decision::insert(nt)).unwrap();
    let id: i64 = db(dir)
        .query_row(
            "SELECT s.id FROM sessions s JOIN boxes b ON b.id = s.box ORDER BY s.id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    set_parent(dir, id, parent);
    id
}

#[test]
fn a_box_session_reports_its_box_and_parent_on_the_wire() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let root = host(&cx, "/r");
    let child = box_child(&cx, dir.path(), "demo", root);
    let value = dispatch(&cx, Request::Sessions(all())).unwrap();
    let rows = value["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["session"], json!(root));
    assert_eq!(rows[0]["box"], json!(null));
    assert_eq!(rows[0]["parent"], json!(null));
    assert_eq!(rows[1]["session"], json!(child));
    assert_eq!(rows[1]["box"], json!("demo"));
    assert_eq!(rows[1]["parent"], json!(root));
}

#[test]
fn the_parent_filter_keeps_the_box_child_with_its_box() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let root = host(&cx, "/r");
    let child = box_child(&cx, dir.path(), "demo", root);
    let req = SessionsReq {
        parent: Some(SessionId::new(root)),
    };
    let value = dispatch(&cx, Request::Sessions(req)).unwrap();
    let rows = value["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["session"], json!(child));
    assert_eq!(rows[0]["box"], json!("demo"));
    assert_eq!(rows[0]["parent"], json!(root));
}
