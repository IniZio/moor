use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moor_core::activity::{PaneObs, PaneStatus};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::engine::{Engine, EngineError, SessionRef, Steer, SteerOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneCmd, Surface, SurfaceError};
use moor_core::{BoxHandle, BoxPath};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::gh_release::fake::offline_release;
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
    let s = FakeSurface::new();
    let ws = s.ensure_workspace("moor:t1").unwrap();
    let pane = s.open_pane(&ws, &PaneCmd::default()).unwrap();
    let s = reads
        .into_iter()
        .fold(s, |s, r| s.script_pane_obs(pane.as_str(), r));
    let session = SessionRef {
        id: "s1".to_owned(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        pane: Some(pane),
        warnings: Vec::new(),
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
        offline_release(),
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
fn an_interrupt_sends_the_profile_key_and_types_nothing() {
    let (s, session) = surface(vec![Ok(obs(""))]);
    let (out, waited) = steer(&s, &session, &Steer::Interrupt);
    assert_eq!(
        out,
        Ok(SteerOutcome {
            submit_retried: false,
            submit_checked: false
        })
    );
    assert_eq!(sends(&s), ["send_keys p-2 esc"]);
    assert_eq!(waited, Duration::ZERO);
}

#[test]
fn an_interrupt_to_a_gone_pane_is_pane_gone() {
    let (s, session) = surface(vec![Err(SurfaceError::NotFound)]);
    let (out, _) = steer(&s, &session, &Steer::Interrupt);
    assert_eq!(out, Err(EngineError::PaneGone));
    assert!(sends(&s).is_empty());
}

#[test]
fn text_clears_types_then_submits_with_the_waits() {
    let (s, session) = surface(vec![Ok(obs("")), Ok(obs("> \n"))]);
    let (out, waited) = steer(&s, &session, &text("fix the build"));
    assert_eq!(
        out,
        Ok(SteerOutcome {
            submit_retried: false,
            submit_checked: true
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
            submit_retried: true,
            submit_checked: true
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
            submit_retried: false,
            submit_checked: true
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
            submit_retried: false,
            submit_checked: false
        })
    );
    assert_eq!(sends(&s), ["send_keys p-2 2"]);
    assert_eq!(waited, Duration::ZERO);
}

/// A clock that logs how many surface calls came before each sleep.
struct LogClock {
    surface: Arc<FakeSurface>,
    time: FakeClock,
    at: Mutex<Vec<Vec<String>>>,
}

impl Clock for LogClock {
    fn now(&self) -> Ts {
        self.time.now()
    }

    fn instant(&self) -> Instant {
        self.time.instant()
    }

    fn sleep(&self, _d: Duration) {
        let calls = self.surface.calls();
        let steer = calls
            .into_iter()
            .skip_while(|c| !c.starts_with("pane_obs"))
            .collect();
        self.at.lock().unwrap().push(steer);
    }
}

#[test]
fn each_wait_comes_before_the_submit_key_and_before_the_re_read() {
    let (s, session) = surface(vec![Ok(obs("")), Ok(obs(""))]);
    let clock = Arc::new(LogClock {
        surface: s.clone(),
        time: FakeClock::new(Ts::from_millis(0)),
        at: Mutex::new(Vec::new()),
    });
    let agent = BoxAgent::new(
        Arc::new(FakeDriver::new()),
        s,
        Arc::new(Claude),
        clock.clone(),
        offline_release(),
    );
    agent.steer(&session, &text("go"), DEADLINE).unwrap();
    let at = clock.at.lock().unwrap();
    assert_eq!(
        *at,
        [
            vec!["pane_obs p-2", "send_keys p-2 ctrl+u", "send_text p-2 go"],
            vec![
                "pane_obs p-2",
                "send_keys p-2 ctrl+u",
                "send_text p-2 go",
                "send_keys p-2 ctrl+m"
            ]
        ]
    );
}

#[test]
fn text_far_above_the_input_is_not_a_pending_submit() {
    let mut tail = String::from("> fix the build\n");
    tail.push_str(&"line\n".repeat(12));
    let (s, session) = surface(vec![Ok(obs("")), Ok(obs(&tail))]);
    let (out, _) = steer(&s, &session, &text("fix the build"));
    assert_eq!(
        out,
        Ok(SteerOutcome {
            submit_retried: false,
            submit_checked: true
        })
    );
    assert_eq!(
        sends(&s).iter().filter(|c| c.ends_with("ctrl+m")).count(),
        1
    );
}

#[test]
fn a_failed_re_read_is_reported_unchecked_without_a_retry() {
    let (s, session) = surface(vec![Ok(obs("")), Err(SurfaceError::Timeout)]);
    let (out, _) = steer(&s, &session, &text("go"));
    assert_eq!(
        out,
        Ok(SteerOutcome {
            submit_retried: false,
            submit_checked: false
        })
    );
    assert_eq!(
        sends(&s).iter().filter(|c| c.ends_with("ctrl+m")).count(),
        1
    );
}

#[test]
fn a_long_message_is_found_by_its_last_characters() {
    let msg = "please look at the failing build and then fix the flaky test";
    let (s, session) = surface(vec![
        Ok(obs("")),
        Ok(obs("> ...uild and then fix the flaky test\n")),
    ]);
    let (out, _) = steer(&s, &session, &text(msg));
    assert!(out.unwrap().submit_retried);
}

#[test]
fn an_upper_case_busy_marker_means_no_retry() {
    let (s, session) = surface(vec![
        Ok(obs("")),
        Ok(obs("> fix the build\nESC TO INTERRUPT\n")),
    ]);
    let (out, _) = steer(&s, &session, &text("fix the build"));
    assert!(!out.unwrap().submit_retried);
}

#[test]
fn an_exit_marker_after_submitting_is_pane_gone() {
    let mut exited = obs("moor: agent exited 2");
    exited.exit_marker = Some(2);
    let (s, session) = surface(vec![Ok(obs("")), Ok(exited)]);
    let (out, _) = steer(&s, &session, &text("go"));
    assert_eq!(out, Err(EngineError::PaneGone));
}

#[test]
fn a_failed_read_before_sending_fails_the_read_step_and_sends_nothing() {
    let (s, session) = surface(vec![Err(SurfaceError::Timeout)]);
    let (out, _) = steer(&s, &session, &text("go"));
    assert_eq!(
        out,
        Err(EngineError::Failed {
            step: "read",
            message: "timed out".to_owned()
        })
    );
    assert!(sends(&s).is_empty());
}

#[test]
fn a_blank_message_is_never_a_pending_submit() {
    let (s, session) = surface(vec![Ok(obs("")), Ok(obs("> \n"))]);
    let (out, _) = steer(&s, &session, &text("  "));
    assert!(!out.unwrap().submit_retried);
}

#[test]
fn a_short_end_of_a_long_message_is_not_a_pending_submit() {
    let msg = "please look at the failing build and then fix the flaky test";
    let (s, session) = surface(vec![Ok(obs("")), Ok(obs("> the flaky test\n"))]);
    let (out, _) = steer(&s, &session, &text(msg));
    assert!(!out.unwrap().submit_retried);
    assert_eq!(
        sends(&s).iter().filter(|c| c.ends_with("ctrl+m")).count(),
        1
    );
}

#[test]
fn the_twelfth_line_from_the_bottom_still_counts() {
    let tail = format!("> fix the build\n{}", "line\n".repeat(11));
    let (s, session) = surface(vec![Ok(obs("")), Ok(obs(&tail))]);
    let (out, _) = steer(&s, &session, &text("fix the build"));
    assert!(out.unwrap().submit_retried);
}

/// A text steer, as MCP `send_to_box` makes one, typed and submitted into a pty pane: no herdr
/// server takes part.
#[cfg(feature = "pty-surface")]
#[test]
fn a_text_steer_reaches_a_pty_pane() {
    use crate::adapters::pty::PtySurface;

    let s = Arc::new(PtySurface::default());
    let ws = s.ensure_workspace("moor:t1").unwrap();
    let cmd = PaneCmd {
        label: "agent".to_owned(),
        argv: vec!["sh".to_owned(), "-c".to_owned(), "exec cat".to_owned()],
        env: Vec::new(),
    };
    let pane = s.open_pane(&ws, &cmd).unwrap();
    let session = SessionRef {
        id: "s1".to_owned(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        pane: Some(pane.clone()),
        warnings: Vec::new(),
    };
    let agent = BoxAgent::new(
        Arc::new(FakeDriver::new()),
        s.clone(),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        offline_release(),
    );
    agent
        .steer(&session, &text("hello from mcp"), DEADLINE)
        .unwrap();
    let mut tail = String::new();
    for _ in 0..500 {
        tail = s.pane_obs(&pane, DEADLINE).unwrap().tail;
        if tail.matches("hello from mcp").count() >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(tail.matches("hello from mcp").count() >= 2, "{tail}");
}
