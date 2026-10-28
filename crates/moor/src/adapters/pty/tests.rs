//! Tests for the pty surface, against real pseudo-terminals.

use std::path::Path;
use std::time::Duration;

use moor_core::activity::{PaneObs, PaneStatus};
use moor_core::name::BoxName;
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{
    Key, PaneCmd, PaneRef, ResumeOutcome, ResumeReg, Surface, SurfaceError,
};

use super::screen::last_lines;
use super::PtySurface;

const DEADLINE: Deadline = Deadline(Duration::from_secs(5));

/// Polls a test makes, 20ms apart, before it gives up (ten seconds).
const POLLS: usize = 500;

fn sh(script: &str) -> PaneCmd {
    PaneCmd {
        label: "agent".to_owned(),
        argv: vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
        env: vec![("MOOR_PTY_TEST".to_owned(), "on".to_owned())],
    }
}

fn open(s: &PtySurface, script: &str) -> PaneRef {
    let ws = s.ensure_workspace("moor:t").expect("workspace");
    s.open_pane(&ws, &sh(script)).expect("pane")
}

/// Observes `pane` until `done` holds, failing with the last tail after ten seconds.
fn obs_until(s: &PtySurface, pane: &PaneRef, done: impl Fn(&PaneObs) -> bool) -> PaneObs {
    let mut obs = s.pane_obs(pane, DEADLINE).expect("obs");
    for _ in 0..POLLS {
        if done(&obs) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        obs = s.pane_obs(pane, DEADLINE).expect("obs");
    }
    assert!(done(&obs), "gave up on tail:\n{}", obs.tail);
    obs
}

#[test]
fn tail_is_the_rendered_screen_after_cursor_moves() {
    let s = PtySurface::default();
    let pane = open(
        &s,
        r"printf 'hello world\nsecond\033[1;7HWORLD\033[3;1Hthird\n'; printf $MOOR_PTY_TEST; exec sleep 600",
    );
    let obs = obs_until(&s, &pane, |o| o.tail.ends_with("on"));
    assert_eq!(obs.tail, "hello WORLD\nsecond\nthird\non");
    assert_eq!(obs.status, PaneStatus::Unknown);
    assert_eq!(obs.prompt, None);
    assert_eq!(obs.exit_marker, None);
}

#[test]
fn exit_marker_is_read_after_the_command_ends() {
    let s = PtySurface::default();
    let pane = open(
        &s,
        r#"printf 'working\r\033[K'; echo "moor: agent exited 3""#,
    );
    let obs = obs_until(&s, &pane, |o| o.exit_marker.is_some());
    assert_eq!(obs.exit_marker, Some(3));
    assert_eq!(obs.tail, "moor: agent exited 3");
}

#[test]
fn keys_and_text_arrive_as_terminal_bytes() {
    let s = PtySurface::default();
    let pane = open(
        &s,
        "stty raw -echo; echo ready; dd bs=1 count=6 2>/dev/null | od -An -tx1; exec sleep 600",
    );
    obs_until(&s, &pane, |o| o.tail.contains("ready"));
    s.send_keys(&pane, &[Key::CtrlU, Key::Digit(7), Key::Escape, Key::CtrlM])
        .expect("keys");
    s.send_text(&pane, "hi").expect("text");
    let obs = obs_until(&s, &pane, |o| o.tail.contains("0d"));
    assert!(obs.tail.contains("15 37 1b 0d 68 69"), "{}", obs.tail);
    assert!(matches!(
        s.send_keys(&pane, &[Key::Digit(12)]),
        Err(SurfaceError::Failed { .. })
    ));
}

#[test]
fn close_pane_kills_its_process() {
    let dir = tempfile::tempdir().expect("dir");
    let pidfile = dir.path().join("pid");
    let s = PtySurface::default();
    let pane = open(
        &s,
        &format!("echo $$ > {}; exec sleep 600", pidfile.display()),
    );
    let pid = wait_for_pid(&pidfile);
    assert!(alive(pid), "pane process should run before close");
    s.close_pane(&pane).expect("close");
    for _ in 0..POLLS {
        if !alive(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(pid), "pane process outlived close");
    assert_eq!(s.pane_obs(&pane, DEADLINE), Err(SurfaceError::NotFound));
    assert_eq!(s.close_pane(&pane), Err(SurfaceError::NotFound));
    assert_eq!(s.send_text(&pane, "x"), Err(SurfaceError::NotFound));
}

#[test]
fn workspaces_are_kept_by_label_and_close_their_panes() {
    let s = PtySurface::default();
    let a = s.ensure_workspace("moor:a").expect("a");
    assert_eq!(s.ensure_workspace("moor:a").expect("a again"), a);
    let b = s.ensure_workspace("moor:b").expect("b");
    assert_ne!(a, b);
    assert_eq!(s.focus_workspace(&a), Ok(()));
    let in_a = s.open_pane(&a, &sh("exec sleep 600")).expect("pane a");
    let in_b = s.open_pane(&b, &sh("exec sleep 600")).expect("pane b");
    s.close_workspace(&a).expect("close a");
    assert_eq!(s.pane_obs(&in_a, DEADLINE), Err(SurfaceError::NotFound));
    assert!(s.pane_obs(&in_b, DEADLINE).is_ok());
    assert_eq!(s.close_workspace(&a), Err(SurfaceError::NotFound));
    assert_ne!(s.ensure_workspace("moor:a").expect("a anew"), a);
    let empty = PaneCmd::default();
    assert!(matches!(
        s.open_pane(&b, &empty),
        Err(SurfaceError::Failed { .. })
    ));
}

#[test]
fn resume_needs_no_registration() {
    let s = PtySurface::default();
    let pane = open(&s, "exec sleep 600");
    let reg = ResumeReg {
        agent: "claude".to_owned(),
        state: PaneStatus::Idle,
        session_id: "agent-1".to_owned(),
        box_name: BoxName::parse_allow_bare("b1").expect("name"),
    };
    assert_eq!(
        s.register_resume(&pane, &reg),
        Ok(ResumeOutcome::Registered)
    );
    s.close_pane(&pane).expect("close");
    assert_eq!(s.register_resume(&pane, &reg), Err(SurfaceError::NotFound));
}

#[test]
fn a_recorded_agent_screen_renders_as_its_alternate_screen() {
    let screen =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/adapters/pty/testdata/claude-screen.ans");
    let s = PtySurface::default();
    let pane = open(&s, &format!("cat '{}'; exec sleep 600", screen.display()));
    let obs = obs_until(&s, &pane, |o| o.tail.contains("shortcuts"));
    let want = "\
╭──────────────────────────────╮
│ ✻ Welcome to Claude Code!    │
╰──────────────────────────────╯

> fix the flaky test

  ● Read 3 files

╭──────────────────────────────╮
│ >                            │
╰──────────────────────────────╯
  ? for shortcuts";
    assert_eq!(obs.tail, want);
}

#[test]
fn last_lines_keeps_the_end_without_trailing_blanks() {
    assert_eq!(last_lines("a  \nb\nc \n\n  \n", 2), "b\nc");
    assert_eq!(last_lines("a\nb", 5), "a\nb");
    assert_eq!(last_lines("\n\n", 3), "");
}

fn wait_for_pid(file: &Path) -> i32 {
    (0..POLLS)
        .find_map(|_| {
            let pid = std::fs::read_to_string(file)
                .ok()
                .and_then(|s| s.trim().parse().ok());
            if pid.is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
            pid
        })
        .expect("no pid written")
}

fn alive(pid: i32) -> bool {
    rustix::process::Pid::from_raw(pid)
        .is_some_and(|p| rustix::process::test_kill_process(p).is_ok())
}
