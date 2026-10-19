//! Tests for the binding operations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::request::{BindSessionReq, SessionBindingReq};
use moor_core::{ErrorKind, Request, SessionId};

use super::{bind, lookup};
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

fn host_session(cx: &Cx) -> SessionId {
    SessionId::new(
        cx.store()
            .unwrap()
            .open_host_session("/home/me/proj", None, None)
            .unwrap(),
    )
}

fn bind_req(key: &str, session: SessionId) -> BindSessionReq {
    BindSessionReq {
        front: "chat".to_owned(),
        key: key.to_owned(),
        session,
        owner: "u1".to_owned(),
    }
}

fn find_req(key: &str) -> SessionBindingReq {
    SessionBindingReq {
        front: "chat".to_owned(),
        key: key.to_owned(),
    }
}

#[test]
fn binding_a_host_session_creates_it_with_no_task() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let s = host_session(&cx);
    let v = bind(&cx, &bind_req("k", s)).unwrap();
    assert!(v.created);
    assert_eq!(v.binding.session, s);
    assert_eq!(v.binding.task, None);
    assert_eq!(v.binding.owner, "u1");
}

#[test]
fn binding_the_same_key_again_keeps_the_original_session() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let (a, b) = (host_session(&cx), host_session(&cx));
    bind(&cx, &bind_req("k", a)).unwrap();
    let v = bind(&cx, &bind_req("k", b)).unwrap();
    assert!(!v.created);
    assert_eq!(v.binding.session, a);
}

#[test]
fn lookup_returns_the_binding_or_none() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let s = host_session(&cx);
    bind(&cx, &bind_req("k", s)).unwrap();
    assert_eq!(lookup(&cx, &find_req("k")).unwrap().unwrap().session, s);
    assert!(lookup(&cx, &find_req("other")).unwrap().is_none());
}

#[test]
fn an_unknown_session_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let err = bind(&cx, &bind_req("k", SessionId::new(9999))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
}

#[test]
fn an_empty_front_or_key_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let s = host_session(&cx);
    assert_eq!(
        bind(&cx, &bind_req("", s)).unwrap_err().kind,
        ErrorKind::Usage
    );
    let mut r = find_req("k");
    r.front.clear();
    assert_eq!(lookup(&cx, &r).unwrap_err().kind, ErrorKind::Usage);
}

#[test]
fn dispatch_binds_and_looks_up_with_null_for_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let s = host_session(&cx);
    let bound = dispatch(&cx, Request::BindSession(bind_req("k", s))).unwrap();
    assert_eq!(bound.pointer("/created"), Some(&true.into()));
    assert_eq!(bound.pointer("/binding/session"), Some(&s.get().into()));
    let found = dispatch(&cx, Request::SessionBinding(find_req("k"))).unwrap();
    assert_eq!(found.pointer("/owner"), Some(&"u1".into()));
    let miss = dispatch(&cx, Request::SessionBinding(find_req("none"))).unwrap();
    assert!(miss.is_null());
}
