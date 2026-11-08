//! An interrupt reaches the agent with its profile's interrupt key, never relaunches it, and
//! never touches a first-run screen.

use moor_core::seam::agent_adapter::FirstRunPrompt;
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::{EngineError, Steer};
use moor_core::ErrorKind;

use super::{blocked, fx_with, interrupt, live, steered, working};
use crate::ops::steer::run;
use crate::testing::agent;
use crate::testing::engine::FakeEngine;

#[test]
fn an_interrupt_reaches_a_working_agent_and_is_recorded() {
    let engine = FakeEngine::new().probes("agent-1", working());
    let f = fx_with(engine, true);
    let v = run(&f.cx, &interrupt()).unwrap();
    assert_eq!(v.sent, "(interrupt)");
    assert!(!v.answered_menu);
    assert_eq!(
        f.engine.steers(),
        [("agent-1".to_owned(), Steer::Interrupt)]
    );
    assert_eq!(steered(&f).len(), 1);
}

#[test]
fn an_interrupt_reaches_a_blocked_agent_without_a_menu_choice() {
    let engine = FakeEngine::new().probes("agent-1", blocked("Allow?"));
    let f = fx_with(engine, true);
    run(&f.cx, &interrupt()).unwrap();
    assert_eq!(
        f.engine.steers(),
        [("agent-1".to_owned(), Steer::Interrupt)]
    );
}

#[test]
fn an_interrupt_never_relaunches_a_gone_agent() {
    let engine = FakeEngine::new()
        .probes("agent-1", working())
        .steer_result(Err(EngineError::AgentGone));
    let f = fx_with(engine, true);
    let err = run(&f.cx, &interrupt()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(f.engine.launches().len(), 0);
    assert!(live(&f), "the session is left as it was");
}

#[test]
fn an_interrupt_is_refused_at_a_first_run_screen_and_sends_nothing() {
    let at_launch = agent::on_screen(
        agent::read(None, Ts::from_millis(0), 0),
        include_str!("../../../adapters/box_agent/claude/fixtures/folder_trust.txt"),
    );
    let engine = FakeEngine::new()
        .prompt("trust this folder", FirstRunPrompt::FolderTrust)
        .probes("agent-1", at_launch);
    let f = fx_with(engine, true);
    let err = run(&f.cx, &interrupt()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(f.engine.steers().is_empty());
}

#[test]
fn an_interrupt_to_an_idle_agent_leaves_its_session_idle() {
    use moor_core::domain::decision::Decision;
    use moor_core::domain::session_state::SessionInput;

    use crate::ops::common::activity::{read, settle, PROBE_DEADLINE};

    let idle = agent::at_turn(agent::idle(Ts::from_millis(0)), 6);
    let f = fx_with(FakeEngine::new().probes("agent-1", idle), true);
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).session(SessionInput::AgentQuiet))
        .unwrap();
    run(&f.cx, &interrupt()).unwrap();
    let row = store.box_row(f.id).unwrap().unwrap();
    let reading = read(&f.cx, store, &row, f.cx.clock().now(), PROBE_DEADLINE).unwrap();
    settle(&f.cx, store, &row, &reading).unwrap();
    let row = store.box_row(f.id).unwrap().unwrap();
    assert!(row.session.is_quiet(), "no turn ran, so none is left open");
    assert!(
        store.unanswered_steer(f.id).unwrap().is_none(),
        "the interrupt is answered at the next read"
    );
}

#[test]
fn an_interrupt_that_leaves_a_blocked_agent_idle_ends_its_turn() {
    use crate::ops::common::activity::{read, settle, PROBE_DEADLINE};

    let asking = agent::at_turn(agent::permission("Allow?", Ts::from_millis(1_000)), 5);
    let f = fx_with(FakeEngine::new().probes("agent-1", asking), true);
    run(&f.cx, &interrupt()).unwrap();
    let after = agent::at_turn(agent::idle(Ts::from_millis(2_000)), 5);
    f.engine.set_probes("agent-1", after);
    let store = f.cx.store().unwrap();
    let row = store.box_row(f.id).unwrap().unwrap();
    let reading = read(&f.cx, store, &row, f.cx.clock().now(), PROBE_DEADLINE).unwrap();
    settle(&f.cx, store, &row, &reading).unwrap();
    assert!(
        store.unanswered_steer(f.id).unwrap().is_none(),
        "the idle status hc set after the interrupt ends the turn"
    );
    let row = store.box_row(f.id).unwrap().unwrap();
    assert!(row.session.is_quiet(), "the Box goes idle");
}
