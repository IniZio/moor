//! The owner's yes or no for a gate that asks, and what it tells the agent and the inbox.
#![allow(
    clippy::indexing_slicing,
    clippy::cognitive_complexity,
    reason = "test assertions index what they just counted, and one test walks a whole story"
)]

use moor_core::gate::{GateDoc, GateLine, GateNext, GateStatus};
use moor_core::inbox::{InboxItemView, InboxState, InboxValue};
use moor_core::progress::{NeedKind, Progress};
use moor_core::request::{BoxTarget, InboxOp};
use moor_core::seam::clock::Ts;
use moor_core::user::{UserId, Viewer};
use moor_core::view::gate::GateAnswerValue;
use moor_core::workflow::{Answer, StepId, StepResult};
use moor_core::{ErrorKind, GateAnswerReq};

use super::fx::{evidence_gate, fx, gate, Fx};
use crate::error::CommandError;
use crate::ops::gate::answer::run;
use crate::testing::cx::REQUESTER_USER;

fn ship(tries: u64) -> GateDoc {
    GateDoc {
        approval: Some(moor_core::gate::Approval::Owner),
        ..evidence_gate("ship", &["proof.md"], &[], tries)
    }
}

/// A Box that asks its owner for `ship`, seen by its owner.
fn owned(tries: u64) -> Fx {
    let mut f = fx(|_| vec![ship(tries)]);
    f.cx = f.cx.with_viewer(Viewer::User(REQUESTER_USER));
    f
}

/// The agent hands in the evidence and goes quiet, so the gate asks the owner.
fn ask(f: &Fx) {
    f.leave(&[("proof.md", 10)]);
    f.report("ship", StepResult::Ok, &["proof.md"]);
    f.quiet();
}

fn answer(f: &Fx, answer: Answer, note: Option<&str>) -> Result<GateAnswerValue, CommandError> {
    run(&f.cx, &req(f, answer, note, None))
}

fn req(f: &Fx, answer: Answer, note: Option<&str>, ask: Option<String>) -> GateAnswerReq {
    GateAnswerReq {
        target: BoxTarget::Id(f.id),
        answer,
        note: note.map(str::to_owned),
        ask,
    }
}

fn open(try_n: u8, next: GateNext) -> GateStatus {
    GateStatus::Open {
        gate: StepId::parse("ship").unwrap(),
        try_n,
        next,
    }
}

fn inbox(f: &Fx) -> Vec<InboxItemView> {
    let store = f.cx.store().unwrap();
    let value = crate::ops::users::inbox::run(store, REQUESTER_USER, &InboxOp::List).unwrap();
    serde_json::from_value::<InboxValue>(value).unwrap().items
}

#[test]
fn an_asking_gate_needs_the_owner_and_a_yes_passes_it() {
    let f = owned(1);
    ask(&f);
    assert_eq!(f.standing(), (open(1, GateNext::Ask), Progress::NeedsYou));
    let card = f.shown().progress.unwrap().needs.unwrap();
    assert_eq!(card.kind, NeedKind::Approve);
    assert!(card.question.contains("ship") && card.question.contains("proof.md"));

    let said = answer(&f, Answer::Yes, None).unwrap();
    assert!(
        said.outcome.contains("every gate has passed"),
        "{}",
        said.outcome
    );
    assert_eq!(f.standing(), (GateStatus::Passed, Progress::Done));
    let lines = f.lines();
    assert!(
        matches!(lines.last(), Some(GateLine::Answered { try_n: 1, answer: Answer::Yes, by, .. }) if !by.is_empty()),
        "{lines:?}"
    );
}

#[test]
fn a_no_with_a_note_steers_the_agent_and_opens_the_next_try() {
    let f = owned(2);
    ask(&f);
    let said = answer(&f, Answer::No, Some("use the blue header")).unwrap();
    assert!(said.outcome.contains("try 2 starts"), "{}", said.outcome);
    assert!(said.outcome.contains("agent was told"), "{}", said.outcome);
    assert_eq!(f.standing().0, open(2, GateNext::Agent));
    let told = f.told();
    assert!(
        told.iter()
            .any(|t| t.contains("use the blue header") && t.contains("did not approve")),
        "{told:?}"
    );
    assert_eq!(told.len(), 1);
    f.pass();
    assert_eq!(f.told().len(), 1, "a pass does not tell it twice");
}

#[test]
fn a_no_on_the_last_try_fails_the_gate_and_the_box_with_the_note() {
    let f = owned(2);
    ask(&f);
    answer(&f, Answer::No, Some("first")).unwrap();
    ask(&f);
    let said = answer(&f, Answer::No, Some("still wrong")).unwrap();
    assert!(said.outcome.contains("failed"), "{}", said.outcome);
    let (status, progress) = f.standing();
    assert!(
        matches!(&status, GateStatus::Failed { why, .. } if why.contains("still wrong")),
        "{status:?}"
    );
    assert_eq!(progress, Progress::Failed);
    let line = f.shown().progress.unwrap().line;
    assert!(line.contains("still wrong"), "{line}");
}

#[test]
fn only_the_owner_answers_and_only_a_gate_that_asks() {
    let mut f = owned(1);
    f.cx = f.cx.with_viewer(Viewer::User(UserId::new(2)));
    ask(&f);
    let err = answer(&f, Answer::Yes, None).unwrap_err();
    assert_eq!(
        (err.kind, err.error.to_string().as_str()),
        (ErrorKind::Refused, "not your Box")
    );
    assert!(
        matches!(f.lines().last(), Some(GateLine::Submitted { .. })),
        "the refused answer wrote nothing"
    );

    f.cx = f.cx.with_viewer(Viewer::User(REQUESTER_USER));
    assert_eq!(f.standing().0, open(1, GateNext::Ask));
    answer(&f, Answer::Yes, None).unwrap();
    let err = answer(&f, Answer::Yes, None).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error
            .to_string()
            .contains("every gate of Box t has passed"),
        "{}",
        err.error
    );
}

#[test]
fn a_gate_that_waits_for_the_agent_is_refused_readably_and_so_is_a_box_without_gates() {
    let f = owned(1);
    let err = answer(&f, Answer::Yes, None).unwrap_err();
    assert!(
        err.error
            .to_string()
            .contains("waits for the agent's evidence"),
        "{}",
        err.error
    );

    let mut plain = fx(|_| vec![gate("tests", &["true"], 1)]);
    plain.cx = plain.cx.with_viewer(Viewer::User(REQUESTER_USER));
    let err = answer(&plain, Answer::No, None).unwrap_err();
    assert!(
        err.error.to_string().contains("waits for its checks"),
        "{}",
        err.error
    );
}

#[test]
fn an_answer_to_an_earlier_question_is_refused_and_a_replayed_no_fails_one_try_only() {
    let f = owned(3);
    ask(&f);
    let first = f.shown().progress.unwrap().needs.unwrap().id;
    answer(&f, Answer::No, None).unwrap();
    ask(&f);
    let second = f.shown().progress.unwrap().needs.unwrap().id;
    assert_ne!(first, second);

    let stale = run(&f.cx, &req(&f, Answer::No, None, Some(first)));
    let err = stale.unwrap_err();
    assert!(
        err.error.to_string().contains("asks something else"),
        "{}",
        err.error
    );
    assert_eq!(f.standing().0, open(2, GateNext::Ask));

    let replay = GateLine::Answered {
        gate: StepId::parse("ship").unwrap(),
        try_n: 1,
        by: "me".to_owned(),
        answer: Answer::No,
        note: String::new(),
    };
    let n = i64::try_from(f.lines().len()).unwrap() + 1;
    f.cx.store()
        .unwrap()
        .append_gate_line(f.id, n, f.cx.clock().now(), &replay)
        .unwrap();
    assert_eq!(
        f.standing().0,
        open(2, GateNext::Ask),
        "an answer of try 1 does not fail try 2"
    );
    run(&f.cx, &req(&f, Answer::Yes, None, Some(second))).unwrap();
    assert_eq!(f.standing().0, GateStatus::Passed);
}

#[test]
fn a_stuck_gate_shows_its_failing_command_and_says_how_to_go_on() {
    let mut f = fx(|dir| {
        let check = format!("echo nope; test -e {}", dir.join("fixed").display());
        vec![gate("tests", &[&check], 1)]
    });
    f.cx = f.cx.with_viewer(Viewer::User(REQUESTER_USER));
    f.quiet();
    f.pass();
    let shown = f.shown().progress.unwrap();
    assert_eq!(shown.state, Progress::NeedsYou);
    let card = shown.needs.unwrap();
    assert_eq!(card.kind, NeedKind::Stuck);
    let wait = card.gate.unwrap();
    assert_eq!(wait.gate, "tests");
    assert!(wait.command.unwrap().starts_with("echo nope; test -e"));
    assert!(wait.why.unwrap().contains("exited 1"));
    assert_eq!(wait.tail.as_deref().map(str::trim), Some("nope"));
    assert!(shown.line.contains("echo nope; test -e"), "{}", shown.line);
    let err = answer(&f, Answer::Yes, None).unwrap_err();
    assert!(
        err.error.to_string().contains("steer its agent to reopen"),
        "{}",
        err.error
    );
}

#[test]
fn the_inbox_gets_one_entry_when_a_gate_starts_waiting_and_it_resolves_with_the_answer() {
    let f = owned(2);
    assert!(inbox(&f).is_empty());
    ask(&f);
    for _ in 0..3 {
        f.pass();
    }
    let items = inbox(&f);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].state, InboxState::Unread);
    assert!(items[0].text.contains("ship"), "{}", items[0].text);

    answer(&f, Answer::No, None).unwrap();
    f.pass();
    let items = inbox(&f);
    assert_eq!((items.len(), items[0].state), (1, InboxState::Resolved));

    ask(&f);
    f.pass();
    let items = inbox(&f);
    assert_eq!(
        items.len(),
        2,
        "the next question is a new entry: {items:?}"
    );
    assert_eq!(items[0].state, InboxState::Unread);
}

#[test]
fn a_gate_that_gets_stuck_writes_one_entry_too() {
    let f = fx(|dir| {
        let check = format!("test -e {}", dir.join("fixed").display());
        vec![gate("tests", &[&check], 1)]
    });
    f.quiet();
    f.pass();
    f.pass();
    f.pass();
    let items = inbox(&f);
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0].text.contains("stuck"), "{}", items[0].text);
}

/// The owner's Box and its machine go to sleep while the gate waits for the answer.
fn sleep(f: &Fx) {
    use moor_core::domain::box_state::BoxLifecycleInput;
    use moor_core::domain::decision::Decision;
    use moor_core::domain::hands::{HandsInc, HandsInput};
    let store = f.cx.store().unwrap();
    let d = Decision::new(f.id)
        .lifecycle(BoxLifecycleInput::sleep(false))
        .hands_input(HandsInc::FIRST, HandsInput::Sleep);
    store.apply(d).unwrap();
}

#[test]
fn a_no_to_an_asleep_box_is_queued_for_the_agent_and_the_inbox_pass_wakes_it() {
    let f = owned(2);
    ask(&f);
    sleep(&f);
    assert_eq!(f.standing(), (open(1, GateNext::Ask), Progress::NeedsYou));
    let said = answer(&f, Answer::No, Some("use the blue header")).unwrap();
    assert!(said.outcome.contains("queued"), "{}", said.outcome);
    assert_eq!(f.standing().0, open(2, GateNext::Agent));
    assert!(
        f.told().is_empty(),
        "the Box sleeps, so nothing is sent yet"
    );

    let idle = crate::testing::agent::idle(Ts::from_millis(i64::MAX / 2));
    f.engine.set_probes("agent-1", idle);
    let backoff = crate::ops::InboxBackoff::default();
    crate::ops::inbox_pass(&f.cx, f.cx.store().unwrap(), &|| false, &backoff).unwrap();
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake ")));
    let told = f.told();
    assert!(
        told.iter().any(|t| t.contains("use the blue header")),
        "{told:?}"
    );
    f.pass();
    assert_eq!(f.told().len(), 1, "a gate pass does not tell it twice");
}

#[test]
fn a_yes_to_an_asleep_box_passes_the_gate_and_wakes_nothing() {
    let f = owned(1);
    ask(&f);
    sleep(&f);
    answer(&f, Answer::Yes, None).unwrap();
    assert_eq!(f.standing().0, GateStatus::Passed);
    assert!(f.told().is_empty());
    assert!(f.driver.calls().iter().all(|c| !c.starts_with("wake ")));
}
