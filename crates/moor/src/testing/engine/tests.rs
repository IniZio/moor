//! Self-tests for the Engine fake.

use std::time::Duration;

use moor_core::activity::Probe;
use moor_core::seam::engine::{
    Engine, EngineError, LaunchPrompt, LaunchReq, SessionRef, Steer, SteerOutcome,
};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath};

use super::FakeEngine;

fn session(id: &str) -> SessionRef {
    SessionRef {
        id: id.to_owned(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work/box-1", "x").unwrap(),
    }
}

const DL: Deadline = Deadline(Duration::from_secs(1));

fn gone() -> Probe {
    crate::testing::agent::gone()
}

#[test]
fn scripted_probes_are_returned_every_time() {
    let e = FakeEngine::new().probes("s1", gone());
    assert_eq!(e.probe(&session("s1"), DL), gone());
    assert_eq!(e.probe(&session("s1"), DL), gone());
}

#[test]
fn unscripted_session_probes_a_live_agent_with_no_status() {
    let e = FakeEngine::new().probes("s1", gone());
    let read = e.probe(&session("other"), DL);
    let term = read.term().expect("a read");
    assert_eq!(term.status, None);
    assert_eq!(term.life, moor_core::activity::Life::Live);
}

#[test]
fn fail_probe_fails_once_then_returns_scripted() {
    let e = FakeEngine::new().probes("s1", gone()).fail_probe("s1");
    assert_eq!(e.probe(&session("s1"), DL), Probe::TimedOut);
    assert_eq!(e.probe(&session("s1"), DL), gone());
    assert_eq!(e.probe_count("s1"), 2);
}

#[test]
fn counts_are_kept_per_session() {
    let e = FakeEngine::new();
    assert_eq!(e.probe_count("a"), 0);
    e.probe(&session("a"), DL);
    e.probe(&session("a"), DL);
    e.probe(&session("b"), DL);
    assert_eq!(e.probe_count("a"), 2);
    assert_eq!(e.probe_count("b"), 1);
    assert_eq!(e.probe_count("c"), 0);
}

fn launch_req() -> LaunchReq {
    LaunchReq {
        profile: None,
        mcp: std::collections::BTreeMap::new(),
        id: BoxId::new(1),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work/box-1", "x").unwrap(),
        box_name: BoxName::parse_allow_bare("t1").unwrap(),
        branch: "moor/t1".to_owned(),
        resume: None,
        issue: None,
        setup_exit: None,
        repo_notes: None,
        disk_note: None,
        shape: None,
        model: None,
        nonce: "abc123".to_owned(),
        proxy: None,
        strip_env: Vec::new(),
        prompt: LaunchPrompt::None,
    }
}

#[test]
fn launches_are_recorded_and_succeed_with_an_empty_session_id_unless_scripted() {
    let e = FakeEngine::new();
    let s = e.launch(&launch_req()).unwrap();
    assert_eq!(s.id, "");
    assert_eq!(e.launches(), [launch_req()]);
}

#[test]
fn scripted_launch_results_are_served_in_order() {
    let err = EngineError::Failed {
        step: "launch",
        message: "x".to_owned(),
    };
    let e = FakeEngine::new().launch_result(Err(err.clone()));
    assert_eq!(e.launch(&launch_req()), Err(err));
    assert!(e.launch(&launch_req()).is_ok());
    assert_eq!(e.launches().len(), 2);
}

#[test]
fn steers_are_recorded_and_served_in_order_then_succeed() {
    let retried = SteerOutcome {
        submit_retried: true,
        submit_checked: true,
    };
    let e = FakeEngine::new()
        .steer_result(Ok(retried))
        .steer_result(Err(EngineError::AgentGone));
    let text = Steer::Text("go".to_owned());
    assert_eq!(e.steer(&session("a"), &text, DL), Ok(retried));
    assert_eq!(
        e.steer(&session("a"), &Steer::Menu(2), DL),
        Err(EngineError::AgentGone)
    );
    assert_eq!(
        e.steer(&session("b"), &text, DL),
        Ok(SteerOutcome {
            submit_retried: false,
            submit_checked: true
        })
    );
    assert_eq!(
        e.steers(),
        [
            ("a".to_owned(), text.clone()),
            ("a".to_owned(), Steer::Menu(2)),
            ("b".to_owned(), text)
        ]
    );
}
