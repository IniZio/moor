//! `show` by Box id next to `show` by name.

use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::{BoxId, BoxName, BoxTarget, ErrorKind};

use super::{add, cx, req};
use crate::cx::Cx;
use crate::ops::show::run;
use crate::testing::driver::FakeDriver;

fn close(cx: &Cx, id: BoxId) {
    let close = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Close)
        .session(SessionInput::End);
    cx.store().unwrap().apply(close).unwrap();
}

/// The id and lifecycle of the one Box `target` shows.
fn shown(cx: &Cx, target: BoxTarget) -> (i64, BoxLifecycle) {
    let r = moor_core::ShowReq {
        target: Some(target),
        ..req(None, None)
    };
    let b = run(cx, &r).unwrap().boxed.unwrap();
    (b.id, b.lifecycle)
}

fn named(s: &str) -> BoxTarget {
    BoxName::parse_allow_bare(s).unwrap().into()
}

#[test]
fn by_id_shows_a_closed_box_while_a_newer_box_holds_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let old = add(&cx, "a");
    close(&cx, old);
    let newer = add(&cx, "a");
    assert_eq!(shown(&cx, old.into()), (old.get(), BoxLifecycle::Closed));
    assert_eq!(
        shown(&cx, named("a")),
        (newer.get(), BoxLifecycle::Checking)
    );
}

#[test]
fn by_name_a_closed_box_is_shown_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "c");
    close(&cx, id);
    assert_eq!(shown(&cx, named("c")), (id.get(), BoxLifecycle::Closed));
}

#[test]
fn an_unknown_id_is_no_box() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let r = moor_core::ShowReq {
        target: Some(BoxId::new(99).into()),
        ..req(None, None)
    };
    let err = run(&cx, &r).unwrap_err();
    assert_eq!(
        (err.kind, err.error.to_string()),
        (ErrorKind::Failed, "no Box id 99".to_owned())
    );
}

#[test]
fn a_name_the_controller_pinned_keeps_meaning_the_box_it_checked() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let setup = cx(dir.path(), &driver);
    let old = add(&setup, "a");
    close(&setup, old);
    let grant = moor_core::request::Grant {
        viewer: moor_core::user::Viewer::Operator,
        pins: [(BoxName::parse_allow_bare("a").unwrap(), old)].into(),
    };
    let pinned = cx(dir.path(), &driver).with_grant(grant);
    add(&setup, "a");
    assert_eq!(
        shown(&pinned, named("a")),
        (old.get(), BoxLifecycle::Closed)
    );
}
