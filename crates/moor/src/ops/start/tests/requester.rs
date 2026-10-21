//! A Box acts for the user of the identity that started it, and only that user.

use std::path::PathBuf;
use std::sync::Arc;

use moor_core::user::Identity;
use moor_core::{ErrorKind, SessionId, StartReq};

use super::{req, world, World};
use crate::cx::Cx;
use crate::ops::start::run;
use crate::testing::cx::REQUESTER_USER;
use crate::testing::driver::FakeDriver;
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};
use crate::testing::surface::FakeSurface;

/// The `user` column of every Session of the Box `id`, in order.
fn session_users(w: &World, id: i64) -> Vec<Option<i64>> {
    let store = w.cx.store().unwrap();
    let reader = store.reader().unwrap();
    let mut stmt = reader
        .prepare("SELECT user FROM sessions WHERE box = ? ORDER BY id")
        .unwrap();
    stmt.query_map([id], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// How many Box rows the store holds, open or not.
fn box_rows(w: &World) -> i64 {
    let store = w.cx.store().unwrap();
    store
        .reader()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM boxes", [], |r| r.get(0))
        .unwrap()
}

/// `w` acting as `who` instead of the enrolled test requester.
fn acting_as(mut w: World, who: &str) -> World {
    w.cx = w.cx.with_requester(Identity::parse(who).unwrap());
    w
}

#[test]
fn a_box_and_its_root_session_record_the_requesters_user() {
    let w = world(FakeDriver::new());
    let v = run(&w.cx, &req("demo")).unwrap();
    let row =
        w.cx.store()
            .unwrap()
            .box_row(moor_core::BoxId::new(v.boxed.id))
            .unwrap();
    assert_eq!(row.unwrap().user, Some(REQUESTER_USER));
    assert_eq!(session_users(&w, v.boxed.id), [Some(REQUESTER_USER.get())]);
}

#[test]
fn a_child_session_leaves_the_user_to_its_box() {
    let w = world(FakeDriver::new());
    let parent =
        w.cx.store()
            .unwrap()
            .open_host_session("/w", None, None)
            .unwrap();
    let child = StartReq {
        parent: Some(SessionId::new(parent)),
        ..req("demo")
    };
    let v = run(&w.cx, &child).unwrap();
    assert_eq!(session_users(&w, v.boxed.id), [None]);
}

#[test]
fn the_proxy_spec_names_the_box_user_and_the_vault_key_path() {
    let fake = FakeProxyHost::new();
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_proxy_host(Arc::new(fake.clone()));
    run(
        &w.cx,
        &StartReq {
            goal: Some("ship it".into()),
            ..req("demo")
        },
    )
    .unwrap();
    let spec = fake
        .calls()
        .into_iter()
        .find_map(|c| match c {
            ProxyHostCall::Ensure(spec) => Some(spec),
            _ => None,
        })
        .unwrap();
    assert_eq!(spec.user, REQUESTER_USER);
    assert_eq!(spec.vault_key, w.cx.vault_key().unwrap());
}

#[test]
fn an_identity_with_no_user_is_refused_and_nothing_is_created() {
    let w = acting_as(world(FakeDriver::new()), "local:2000");
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    let message = err.error.to_string();
    assert!(message.starts_with("log in first"), "{message}");
    assert_eq!(box_rows(&w), 0);
    assert!(w.driver.calls().is_empty());
    assert!(w.surface.calls().is_empty());
}

#[test]
fn a_start_outside_a_controller_socket_acts_as_no_one_and_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cx = Cx::fake_bare(
        dir.path(),
        &PathBuf::from("/repo"),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new(dir.path())),
        Arc::new(crate::testing::proc::ScriptedProc::new()),
    );
    let err = run(&cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(
        err.error.to_string().contains("controller socket"),
        "{err:?}"
    );
}

#[test]
fn starting_another_users_open_box_is_refused() {
    let w = world(FakeDriver::new());
    run(&w.cx, &req("demo")).unwrap();
    let other = Identity::parse("local:2000").unwrap();
    w.cx.store().unwrap().enroll(&other).unwrap();
    let w = acting_as(w, "local:2000");
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error.to_string().contains("acts for another user"),
        "{err:?}"
    );
}
