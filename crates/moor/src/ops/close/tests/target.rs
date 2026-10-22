//! `close` by Box id next to `close` by name.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::{BoxId, BoxName, BoxTarget, CloseReq, ErrorKind};

use super::{fx, row, Fx};
use crate::ops::close::run;

fn close(target: BoxTarget) -> CloseReq {
    CloseReq {
        target,
        force: false,
    }
}

fn named_t() -> BoxTarget {
    BoxName::parse_allow_bare("t").unwrap().into()
}

/// A newer open Box named `t`, without a machine; names are unique only among open Boxes.
fn newer_t(f: &Fx) -> BoxId {
    f.cx.store()
        .unwrap()
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("t").unwrap(),
            repo: None,
        }))
        .unwrap()
        .box_id
}

fn lifecycle(f: &Fx, id: BoxId) -> BoxLifecycle {
    f.cx.store()
        .unwrap()
        .box_row(id)
        .unwrap()
        .unwrap()
        .lifecycle
}

#[test]
fn by_id_closes_an_open_box() {
    let f = fx();
    let v = run(&f.cx, &close(f.id.into())).unwrap();
    assert_eq!(v.boxed.id, f.id.get());
    assert_eq!(row(&f).lifecycle, BoxLifecycle::Closed);
}

#[test]
fn a_closed_box_meets_the_same_gate_by_id_as_by_name() {
    let f = fx();
    run(&f.cx, &close(named_t())).unwrap();
    let by_name = run(&f.cx, &close(named_t())).unwrap_err();
    let by_id = run(&f.cx, &close(f.id.into())).unwrap_err();
    assert_eq!(by_name.kind, ErrorKind::Refused);
    assert!(
        by_name.error.to_string().contains("already closed"),
        "{}",
        by_name.error
    );
    assert_eq!(
        (by_id.kind, by_id.error.to_string()),
        (by_name.kind, by_name.error.to_string())
    );
}

#[test]
fn by_id_a_closed_box_is_refused_while_a_newer_open_box_holds_the_name() {
    let f = fx();
    run(&f.cx, &close(f.id.into())).unwrap();
    let newer = newer_t(&f);
    let e = run(&f.cx, &close(f.id.into())).unwrap_err();
    assert!(
        e.error.to_string().contains("already closed"),
        "{}",
        e.error
    );
    assert_eq!(lifecycle(&f, newer), BoxLifecycle::Checking);
}

#[test]
fn an_unknown_id_is_no_box() {
    let f = fx();
    let e = run(&f.cx, &close(BoxId::new(99).into())).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert_eq!(e.error.to_string(), "no Box id 99");
}
