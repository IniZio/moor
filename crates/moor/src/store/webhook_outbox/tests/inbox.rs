//! The inbox fills in the event's own transaction: a question, an approve wait and a failed run
//! each add one row for the Box's owner, and nothing else does.
#![allow(clippy::indexing_slicing, reason = "test assertions index listed rows")]

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::SteerSent;
use moor_core::inbox::InboxKind;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{Plan, RunOutcome, RunState};

use super::runs::{new_run, plan, repo, step};
use super::{insert, world};

#[test]
fn a_question_adds_one_row_for_the_owner_of_the_box_and_nothing_else_does() {
    let w = world();
    let (alice, bob) = (w.user("local:2000"), w.user("local:2001"));
    let boxed = w.store.apply(insert("a").requester(alice)).unwrap().box_id;
    let ask = || {
        let asked = ExtraEvent::NeedsInput {
            question: Some("Allow npm run e2e?".to_owned()),
            options: vec![],
        };
        w.store.apply(Decision::new(boxed).event(asked)).unwrap()
    };
    let event = ask().events[0];
    let rows = w.store.user_inbox(alice, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].kind, rows[0].event),
        (InboxKind::NeedsInput, event)
    );
    assert_eq!(
        (rows[0].text.as_str(), rows[0].read),
        ("Allow npm run e2e?", false)
    );
    assert_eq!(rows[0].box_name, "a");
    assert!(w.store.user_inbox(bob, 10).unwrap().is_empty());
    ask();
    assert_eq!(w.store.user_inbox(alice, 10).unwrap().len(), 2);
    w.new_box("b", alice);
    assert_eq!(w.store.user_inbox(alice, 10).unwrap().len(), 2);
}

#[test]
fn the_question_marker_moves_past_a_question_once_an_answer_is_sent() {
    let w = world();
    let alice = w.user("local:2000");
    let boxed = w.store.apply(insert("a").requester(alice)).unwrap().box_id;
    let asked = ExtraEvent::NeedsInput {
        question: None,
        options: vec![],
    };
    let event = w
        .store
        .apply(Decision::new(boxed).event(asked))
        .unwrap()
        .events[0];
    assert_eq!(w.store.newest_question_marker(boxed).unwrap(), Some(event));
    let doing = ExtraEvent::AgentDoing {
        text: "Read 2 files".to_owned(),
    };
    w.store.apply(Decision::new(boxed).event(doing)).unwrap();
    assert_eq!(w.store.newest_question_marker(boxed).unwrap(), Some(event));
    let answer = SteerSent {
        session: "s".to_owned(),
        message: "1".to_owned(),
        submit_retried: false,
        turn_before: None,
        status_before: None,
        inbox: None,
    };
    let idle = w
        .store
        .apply(Decision::new(boxed).event(ExtraEvent::Steered(answer)))
        .unwrap();
    assert_eq!(
        w.store.newest_question_marker(boxed).unwrap(),
        Some(idle.events[0])
    );
}

#[test]
fn reading_marks_only_the_callers_items_and_an_empty_list_marks_all() {
    let w = world();
    let (alice, bob) = (w.user("local:2000"), w.user("local:2001"));
    for (name, who) in [("a", alice), ("b", alice), ("c", bob)] {
        let boxed = w.store.apply(insert(name).requester(who)).unwrap().box_id;
        let asked = ExtraEvent::NeedsInput {
            question: None,
            options: vec![],
        };
        w.store.apply(Decision::new(boxed).event(asked)).unwrap();
    }
    let mine = w.store.user_inbox(alice, 10).unwrap();
    let theirs = w.store.user_inbox(bob, 10).unwrap();
    assert_eq!(w.store.mark_inbox_read(alice, &[theirs[0].id]).unwrap(), 0);
    assert_eq!(w.store.mark_inbox_read(alice, &[mine[0].id]).unwrap(), 1);
    let read: Vec<bool> = w
        .store
        .user_inbox(alice, 10)
        .unwrap()
        .iter()
        .map(|r| r.read)
        .collect();
    assert_eq!(read, [true, false]);
    assert_eq!(w.store.mark_inbox_read(alice, &[]).unwrap(), 1);
    assert!(w
        .store
        .user_inbox(alice, 10)
        .unwrap()
        .iter()
        .all(|r| r.read));
    assert!(!w.store.user_inbox(bob, 10).unwrap()[0].read);
}

#[test]
#[allow(
    clippy::cognitive_complexity,
    reason = "one scenario, asserted step by step"
)]
fn an_approve_wait_adds_one_row_and_stops_waiting_when_answered_and_a_failed_run_adds_one() {
    let w = world();
    let alice = w.user("local:2000");
    let (id, boxed) = w.workflow(alice, true);
    w.start(id, boxed, 100, Some(plan()));
    let key = (id, &repo(), Ts::from_millis(100));
    let (from, ask, ship) = (Plan::single_id(), step("ask"), step("ship"));
    let wait = || {
        let at = Ts::from_millis(500);
        w.store
            .wait_workflow_run(key, ((&from, 1), (&ask, 1)), boxed, (&[], at))
            .unwrap()
    };
    assert!(wait());
    assert!(!wait());
    let rows = w.store.user_inbox(alice, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, InboxKind::RunWaiting);
    assert_eq!(rows[0].title, "deps (run 1)");
    assert_eq!(rows[0].text, "ship the upgrade?");
    assert_eq!(rows[0].created_at, 500);
    let run = serde_json::from_str::<serde_json::Value>(&rows[0].payload).unwrap()["run"]
        .as_i64()
        .unwrap();
    assert!(w.store.run_waits_since(run, 500).unwrap());
    assert!(!w.store.run_waits_since(run, 499).unwrap());
    let running = RunState::Running {
        box_id: boxed,
        at: ship.clone(),
        try_n: 1,
    };
    let answered = w
        .store
        .answer_workflow_run(key, (&ask, 1), &running, (&[], Ts::from_millis(600)))
        .unwrap();
    assert!(answered);
    assert!(!w.store.run_waits_since(run, 500).unwrap());
    let end = w.store.end_workflow_run(
        key,
        (&ship, 1),
        (boxed, RunOutcome::TimedOut { at: ship.clone() }),
        (&[], Ts::from_millis(900)),
    );
    assert!(end.unwrap());
    let rows = w.store.user_inbox(alice, 10).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].kind, InboxKind::RunFailed);
    assert!(rows[0].text.contains("timeout"), "{}", rows[0].text);
    let _ = new_run;
}
