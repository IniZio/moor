//! A row whose send may have reached the agent is never typed blind again: it is sent once more
//! to the same launch, named to the next one, or reported.

use moor_core::domain::inbox::{Delivery as Row, InboxKind};
use moor_core::seam::engine::EngineError;

use super::queue::{pass, reports_idle};
use super::{fx, fx_with, text};
use crate::ops::steer::{deliver, run};
use crate::testing::engine::FakeEngine;

#[test]
fn a_row_in_doubt_on_an_ended_launch_waits_for_the_next_launch_unreported() {
    let f = fx();
    let store = f.cx.store().unwrap();
    let old = store.live_launch(f.id).unwrap().unwrap();
    let row = store
        .enqueue(f.id, InboxKind::Steer, "cut", "steer")
        .unwrap();
    store.mark_sending(f.id, row, old.id).unwrap();
    store.end_launch_with(old.id, None, None).unwrap();
    assert!(deliver(&f.cx, store, f.id).unwrap().is_empty());
    let left = store.unsent(f.id).unwrap();
    assert!(
        matches!(left.first().map(|r| r.delivery), Some(Row::InDoubt { .. })),
        "{left:?}"
    );
    assert_eq!(
        store.newest_event_of(f.id, "steer_unconfirmed").unwrap(),
        None
    );
}

#[test]
fn an_exit_whose_send_failed_is_reported_and_never_sent_again() {
    let engine = FakeEngine::new().steer_result(Err(EngineError::Failed {
        step: "type",
        message: "pane busy".into(),
    }));
    let f = fx_with(engine, true);
    reports_idle(&f, "agent-1");
    run(&f.cx, &text("/exit")).unwrap_err();
    let store = f.cx.store().unwrap();
    assert!(store.unsent(f.id).unwrap().is_empty());
    assert!(store
        .newest_event_of(f.id, "steer_unconfirmed")
        .unwrap()
        .is_some());
    pass(&f);
    assert_eq!(f.engine.steers().len(), 1, "/exit is typed once");
}
