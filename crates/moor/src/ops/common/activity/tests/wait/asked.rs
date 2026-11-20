//! An agent that ended its turn asking the person needs them, whatever background work it left
//! running: the work shows beside the question and does not keep the Box working.

use moor_core::activity::{WaitCheck, STALL_AFTER};
use moor_core::config::UserConfig;
use moor_core::progress::{derive, Progress};

use super::super::*;
use crate::ops::common::view::{agent_view, box_view};
use crate::testing::agent::replied_beside;

const JOB: &str = "sleep 900";
const REPLY: &str =
    "The stack is up.\n\nShip the fix? Recommend: yes\n\nPrinciples: Prove It Works.";

fn settled(said: &str, check: WaitCheck, at: Ts) -> (tempfile::TempDir, Cx, BoxId, Reading) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, replied_beside(said, &[JOB], seen())));
    engine.set_wait_check(SESSION, check);
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let store = cx.store().unwrap();
    let r = read(&cx, store, &row(&cx, id), at, PROBE_DEADLINE).unwrap();
    settle(&cx, store, &row(&cx, id), &r).unwrap();
    (dir, cx, id, r)
}

#[test]
fn a_turn_that_ends_asking_beside_a_running_job_needs_you_not_working() {
    let (_d, cx, id, r) = settled(REPLY, WaitCheck::Running, NOW);

    let asked = AgentState::Asked {
        question: "Ship the fix?".to_owned(),
        background: vec![JOB.to_owned()],
    };
    assert_eq!(state(&r), (asked, Source::Status));
    assert!(!r.busy());
    assert!(is_idle(&row(&cx, id)), "{:?}", row(&cx, id).session);
    let events = cx.store().unwrap().events(id, 100).unwrap();
    let ev = events.iter().find(|e| e.kind == "needs_input").unwrap();
    assert_eq!(
        ev.payload,
        serde_json::json!({"question": "Ship the fix?", "options": []})
    );

    let agent = agent_view(&r).unwrap();
    assert_eq!(agent.state, "needs_input");
    assert_eq!(agent.question.as_deref(), Some("Ship the fix?"));
    assert_eq!(agent.waiting_on, [JOB]);
    let mut shown = box_view(&row(&cx, id), &UserConfig::default());
    shown.agent = Some(agent);
    let progress = derive(&shown, &[], NOW.as_millis());
    assert_eq!(progress.state, Progress::NeedsYou);
    assert_eq!(progress.line, "Ship the fix?");
}

#[test]
fn a_turn_that_ends_without_a_question_still_waits_on_its_job() {
    let (_d, cx, id, r) = settled("Both are running.", WaitCheck::Running, NOW);
    let waiting = AgentState::Waiting {
        on: vec![JOB.to_owned()],
    };
    assert_eq!(state(&r).0, waiting);
    assert!(r.busy());
    assert!(is_working(&row(&cx, id)));
}

#[test]
fn a_job_that_ends_drops_from_the_card_and_the_question_stands() {
    let late = NOW.saturating_add(STALL_AFTER);
    let (_d, cx, id, r) = settled(REPLY, WaitCheck::Nothing, late);
    let asked = AgentState::Asked {
        question: "Ship the fix?".to_owned(),
        background: Vec::new(),
    };
    assert_eq!(state(&r).0, asked);
    assert!(is_idle(&row(&cx, id)));
}
