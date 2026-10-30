//! An interrupt reaches the agent with its profile's interrupt key, never relaunches it, and
//! never touches a first-run screen.

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::seam::agent_adapter::FirstRunPrompt;
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::{EngineError, Steer};
use moor_core::ErrorKind;

use super::{blocked, fx_with, interrupt, live, steered, working};
use crate::ops::steer::run;
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
        .steer_result(Err(EngineError::PaneGone));
    let f = fx_with(engine, true);
    let err = run(&f.cx, &interrupt()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(f.engine.launches().len(), 0);
    assert!(live(&f), "the session is left as it was");
}

#[test]
fn an_interrupt_is_refused_at_a_first_run_screen_and_sends_nothing() {
    let at_launch = Probes {
        hook: HookRead::Fresh(HookRecord {
            kind: HookKind::Starting { launch: None },
            session_id: None,
            message: None,
            options: Vec::new(),
            doing: None,
            at: Ts::from_millis(0),
            turn: None,
        }),
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: include_str!("../../../adapters/box_agent/claude/fixtures/folder_trust.txt")
                .to_owned(),
            prompt: None,
            exit_marker: None,
        }),
    };
    let engine = FakeEngine::new()
        .hooks_quiet_during_first_run()
        .prompt("trust this folder", FirstRunPrompt::FolderTrust)
        .probes("agent-1", at_launch);
    let f = fx_with(engine, true);
    let err = run(&f.cx, &interrupt()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(f.engine.steers().is_empty());
}
