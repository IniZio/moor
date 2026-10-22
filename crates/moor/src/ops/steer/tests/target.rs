//! `steer` by Box id next to `steer` by name.

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::engine::Steer;
use moor_core::{BoxId, BoxName, BoxTarget, ErrorKind, SteerReq};

use super::{fx, fx_with, text, working, Fx};
use crate::ops::steer::run;
use crate::testing::engine::FakeEngine;

fn to(target: BoxTarget) -> SteerReq {
    SteerReq {
        target,
        ..text("hi")
    }
}

/// Closes the fixture's Box `t`, then opens a newer Box with the same name.
fn close_and_reuse_the_name(f: &Fx) -> BoxId {
    let store = f.cx.store().unwrap();
    let close = Decision::new(f.id)
        .lifecycle(BoxLifecycleInput::Close)
        .session(SessionInput::End);
    store.apply(close).unwrap();
    store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("t").unwrap(),
            repo: None,
        }))
        .unwrap()
        .box_id
}

#[test]
fn by_id_sends_to_that_box() {
    let f = fx_with(FakeEngine::new().probes("agent-1", working()), true);
    let v = run(&f.cx, &to(f.id.into())).unwrap();
    assert_eq!(v.box_name, "t");
    assert_eq!(
        f.engine.steers(),
        vec![("agent-1".to_owned(), Steer::Text("hi".into()))]
    );
}

#[test]
fn by_id_a_closed_box_is_refused_while_a_newer_box_holds_its_name() {
    let f = fx();
    close_and_reuse_the_name(&f);
    let by_id = run(&f.cx, &to(f.id.into())).unwrap_err();
    assert_eq!(by_id.kind, ErrorKind::Failed);
    assert_eq!(
        by_id.error.to_string(),
        format!("no open Box id {}", f.id.get())
    );
    let by_name = run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(by_name.kind, ErrorKind::Refused);
    assert!(
        by_name
            .error
            .to_string()
            .starts_with("no live agent session for Box t"),
        "{}",
        by_name.error
    );
    assert!(f.engine.steers().is_empty());
}

#[test]
fn by_name_a_closed_box_is_refused_as_before() {
    let f = fx();
    let close = Decision::new(f.id)
        .lifecycle(BoxLifecycleInput::Close)
        .session(SessionInput::End);
    f.cx.store().unwrap().apply(close).unwrap();
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(
        (err.kind, err.error.to_string()),
        (ErrorKind::Failed, "no open Box t".to_owned())
    );
}
