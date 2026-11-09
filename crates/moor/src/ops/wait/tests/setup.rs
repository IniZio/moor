//! Tests for `wait` on a Box that is not set up yet.

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::{Decision, Field, HandsFields};
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::{BoxId, ErrorKind, ProviderId, WaitFor};

use super::{add, cx, req};
use crate::cx::Cx;
use crate::ops::wait::run;

/// Moves Box `id` from checking into setup, the way an admitted start does.
fn admit(cx: &Cx, id: BoxId) {
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .hands_input(HandsInc::FIRST, HandsInput::Requested)
        .hands(
            HandsInc::FIRST,
            HandsFields {
                provider: Field::Set(ProviderId::new("p")),
                ..HandsFields::default()
            },
        );
    cx.store().unwrap().apply(d).unwrap();
}

#[test]
fn idle_waits_while_the_box_sets_up() {
    for want in [None, Some(WaitFor::Done), Some(WaitFor::Input)] {
        let dir = tempfile::tempdir().unwrap();
        let (cx, _) = cx(dir.path());
        let id = add(&cx);
        let v = run(&cx, &req(want, Some(1))).unwrap();
        assert!(v.timed_out, "{want:?}: a checking Box has no agent yet");
        admit(&cx, id);
        let v = run(&cx, &req(want, Some(1))).unwrap();
        assert!(v.timed_out, "{want:?}: a Box in setup has no agent yet");
        assert_eq!(v.boxed.lifecycle, BoxLifecycle::SettingUp);
    }
}

#[test]
fn run_does_not_wait_for_setup() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path());
    let id = add(&cx);
    admit(&cx, id);
    let v = run(&cx, &req(Some(WaitFor::Run), Some(5))).unwrap();
    assert!(!v.timed_out);
}

#[test]
fn idle_fails_for_a_box_whose_setup_failed() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path());
    let id = add(&cx);
    admit(&cx, id);
    let d = Decision::new(id).lifecycle(BoxLifecycleInput::SetupFailed {
        cause: "setup exited 1".into(),
        which: None,
    });
    cx.store().unwrap().apply(d).unwrap();
    let e = run(&cx, &req(None, Some(5))).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().contains("setup failed"), "{}", e.error);
    assert_eq!(
        e.value
            .unwrap()
            .pointer("/lifecycle")
            .and_then(|v| v.as_str()),
        Some("setup_failed")
    );
}
