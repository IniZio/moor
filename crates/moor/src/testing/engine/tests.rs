//! Self-tests for the Engine fake.

use std::path::PathBuf;
use std::time::Duration;

use moor_core::activity::{HookRead, PaneRead, Probes};
use moor_core::seam::driver::DriverError;
use moor_core::seam::engine::{Engine, EngineError, LaunchReq, SessionRef, Steer, SteerOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneRef, WorkspaceRef};
use moor_core::{BoxHandle, BoxPath, TaskName};

use super::FakeEngine;

fn session(id: &str) -> SessionRef {
    SessionRef {
        id: id.to_owned(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work/box-1", "x").unwrap(),
        worktree: PathBuf::from("/host/wt"),
        pane: None,
    }
}

const DL: Deadline = Deadline(Duration::from_secs(1));

fn gone() -> Probes {
    Probes {
        hook: HookRead::TimedOut,
        pane: PaneRead::Gone,
    }
}

fn absent() -> Probes {
    Probes {
        hook: HookRead::Absent,
        pane: PaneRead::Absent,
    }
}

#[test]
fn scripted_probes_are_returned_every_time() {
    let e = FakeEngine::new().probes("s1", gone());
    assert_eq!(e.probe(&session("s1"), DL), Ok(gone()));
    assert_eq!(e.probe(&session("s1"), DL), Ok(gone()));
}

#[test]
fn unscripted_session_probes_absent() {
    let e = FakeEngine::new().probes("s1", gone());
    assert_eq!(e.probe(&session("other"), DL), Ok(absent()));
}

#[test]
fn fail_probe_fails_once_then_returns_scripted() {
    let err = EngineError::Box(DriverError::NotFound);
    let e = FakeEngine::new()
        .probes("s1", gone())
        .fail_probe("s1", err.clone());
    assert_eq!(e.probe(&session("s1"), DL), Err(err));
    assert_eq!(e.probe(&session("s1"), DL), Ok(gone()));
    assert_eq!(e.probe_count("s1"), 2);
}

#[test]
fn counts_are_kept_per_session() {
    let e = FakeEngine::new();
    assert_eq!(e.probe_count("a"), 0);
    e.probe(&session("a"), DL).unwrap();
    e.probe(&session("a"), DL).unwrap();
    e.probe(&session("b"), DL).unwrap();
    assert_eq!(e.probe_count("a"), 2);
    assert_eq!(e.probe_count("b"), 1);
    assert_eq!(e.probe_count("c"), 0);
}

fn launch_req() -> LaunchReq {
    LaunchReq {
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work/box-1", "x").unwrap(),
        worktree: PathBuf::from("/host/wt"),
        workspace: WorkspaceRef::new("ws-1"),
        task: TaskName::parse("t1").unwrap(),
        branch: "moor/t1".to_owned(),
        goal: None,
        resume: None,
        issue: None,
        setup_exit: None,
        model: None,
        nonce: "abc123".to_owned(),
        proxy: None,
        origin: None,
    }
}

#[test]
fn launches_are_recorded_and_succeed_with_a_pane_unless_scripted() {
    let e = FakeEngine::new();
    let s = e.launch(&launch_req()).unwrap();
    assert_eq!(s.pane, Some(PaneRef::new("p-1")));
    assert_eq!(e.launches(), [launch_req()]);
}

#[test]
fn scripted_launch_results_are_served_in_order() {
    let err = EngineError::Failed {
        step: "pane",
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
        .steer_result(Err(EngineError::PaneGone));
    let text = Steer::Text("go".to_owned());
    assert_eq!(e.steer(&session("a"), &text, DL), Ok(retried));
    assert_eq!(
        e.steer(&session("a"), &Steer::Menu(2), DL),
        Err(EngineError::PaneGone)
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
