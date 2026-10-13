use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{PaneObs, PaneStatus};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::engine::{Engine, EngineError, SessionRef, Steer, SteerOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneCmd, Surface, SurfaceError};
use moor_core::{BoxHandle, BoxPath, TaskName};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

const DEADLINE: Deadline = Deadline(Duration::from_secs(3));

fn obs(tail: &str) -> PaneObs {
    PaneObs {
        status: PaneStatus::Idle,
        tail: tail.to_owned(),
        prompt: None,
        exit_marker: None,
    }
}

/// A surface with an open pane `p-2` that reads as `reads` in turn.
fn surface(reads: Vec<Result<PaneObs, SurfaceError>>) -> (Arc<FakeSurface>, SessionRef) {
    let s = FakeSurface::new("/r");
    let wt = s
        .create_worktree(Path::new("/r"), &TaskName::parse("t1").unwrap(), None)
        .unwrap();
    let ws = s.ensure_workspace(&wt, "moor:t1").unwrap();
    let pane = s.open_pane(&ws, &PaneCmd::default()).unwrap();
    let s = reads
        .into_iter()
        .fold(s, |s, r| s.script_pane_obs(pane.as_str(), r));
    let session = SessionRef {
        id: "s1".to_owned(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        worktree: PathBuf::from("/host/wt"),
        pane: Some(pane),
    };
    (Arc::new(s), session)
}

fn steer(
    s: &Arc<FakeSurface>,
    session: &SessionRef,
    what: &Steer,
) -> (Result<SteerOutcome, EngineError>, Duration) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let start = clock.instant();
    let agent = BoxAgent::new(
        Arc::new(FakeDriver::new()),
        s.clone(),
        Arc::new(Claude),
        clock.clone(),
    );
    let out = agent.steer(session, what, DEADLINE);
    (out, clock.instant() - start)
}

fn text(t: &str) -> Steer {
    Steer::Text(t.to_owned())
}

fn sends(s: &FakeSurface) -> Vec<String> {
    s.calls()
        .into_iter()
        .filter(|c| c.starts_with("send_"))
        .collect()
}

#[test]
fn text_clears_types_then_submits_with_the_waits() {
    let (s, session) = surface(vec![Ok(obs("")), Ok(obs("> \n"))]);
    let (out, waited) = steer(&s, &session, &text("fix the build"));
    assert_eq!(
        out,
        Ok(SteerOutcome {
            submit_retried: false
        })
    );
    assert_eq!(
        sends(&s),
        [
            "send_keys p-2 ctrl+u",
            "send_text p-2 fix the build",
            "send_keys p-2 ctrl+m"
        ]
    );
    assert_eq!(waited, Duration::from_millis(600));
}

#[test]
fn text_still_in_the_input_is_submitted_once_more() {
    let (s, session) = surface(vec![Ok(obs("")), Ok(obs("> fix the build\n"))]);
    let (out, _) = steer(&s, &session, &text("fix the build"));
    assert_eq!(
        out,
        Ok(SteerOutcome {
            submit_retried: true
        })
    );
    let calls = sends(&s);
    assert_eq!(
        calls.iter().filter(|c| c.starts_with("send_text")).count(),
        1
    );
    assert_eq!(calls.iter().filter(|c| c.ends_with("ctrl+m")).count(), 2);
    assert_eq!(
        calls.last().map(String::as_str),
        Some("send_keys p-2 ctrl+m")
    );
}

#[test]
fn a_busy_marker_means_no_retry() {
    let (s, session) = surface(vec![
        Ok(obs("")),
        Ok(obs("> fix the build\n* Working (esc to interrupt)\n")),
    ]);
    let (out, _) = steer(&s, &session, &text("fix the build"));
    assert_eq!(
        out,
        Ok(SteerOutcome {
            submit_retried: false
        })
    );
    assert_eq!(
        sends(&s).iter().filter(|c| c.ends_with("ctrl+m")).count(),
        1
    );
}

#[test]
fn a_gone_pane_is_pane_gone_and_nothing_is_sent() {
    let (s, session) = surface(vec![Err(SurfaceError::NotFound)]);
    let (out, _) = steer(&s, &session, &text("x"));
    assert_eq!(out, Err(EngineError::PaneGone));
    assert!(sends(&s).is_empty());
}

#[test]
fn an_exited_pane_is_pane_gone_and_nothing_is_sent() {
    let mut exited = obs("moor: agent exited 1");
    exited.exit_marker = Some(1);
    let (s, session) = surface(vec![Ok(exited)]);
    let (out, _) = steer(&s, &session, &Steer::Menu(1));
    assert_eq!(out, Err(EngineError::PaneGone));
    assert!(sends(&s).is_empty());
}

#[test]
fn a_pane_that_vanishes_during_the_steer_is_pane_gone() {
    let (s, session) = surface(vec![Ok(obs("")), Err(SurfaceError::NotFound)]);
    let (out, _) = steer(&s, &session, &text("x"));
    assert_eq!(out, Err(EngineError::PaneGone));
}

#[test]
fn a_session_without_a_pane_has_no_session() {
    let (s, mut session) = surface(vec![]);
    session.pane = None;
    let (out, _) = steer(&s, &session, &text("x"));
    assert_eq!(out, Err(EngineError::NoSession));
}

#[test]
fn menu_sends_the_choice_key() {
    let (s, session) = surface(vec![Ok(obs(""))]);
    let (out, waited) = steer(&s, &session, &Steer::Menu(2));
    assert_eq!(
        out,
        Ok(SteerOutcome {
            submit_retried: false
        })
    );
    assert_eq!(sends(&s), ["send_keys p-2 2"]);
    assert_eq!(waited, Duration::ZERO);
}
