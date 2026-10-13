//! Self-tests for the Engine fake.

use std::path::PathBuf;
use std::time::Duration;

use moor_core::activity::{HookRead, PaneRead, Probes};
use moor_core::seam::driver::DriverError;
use moor_core::seam::engine::{Engine, EngineError, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath};

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
