//! Tests for streamed children using `sh -c`.
#![expect(
    clippy::disallowed_methods,
    reason = "tests time real elapsed durations"
)]

use std::io::{self, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moor_core::seam::proc::{Deadline, Output, Proc, ProcError, ProcSpec, StderrGate, Tool};
use rustix::process::{getpid, kill_process, Signal};
use tempfile::TempDir;

use super::super::{SystemProc, ToolPaths};
use super::Signals;
use crate::env::Env;

fn sh_proc() -> SystemProc {
    let env = Env::from_pairs([("MOOR_SH_BIN".to_owned(), "/bin/sh".to_owned())]);
    SystemProc::new(&env, &ToolPaths::default())
}

fn sh(script: &str, secs: u64) -> ProcSpec {
    ProcSpec::new(Tool::Sh, Deadline(Duration::from_secs(secs)))
        .arg("-c")
        .arg(script)
}

fn wait_for(path: &Path) {
    let start = Instant::now();
    while !path.exists() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(path.exists(), "{} never appeared", path.display());
}

#[test]
fn log_receives_both_streams_and_zero_exit() {
    let dir = TempDir::new().unwrap();
    let log = dir.path().join("run.log");
    sh_proc()
        .run_streamed(&sh("echo out; echo err >&2", 5), &Output::Log(log.clone()))
        .unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("out") && text.contains("err"), "{text}");
}

#[test]
fn log_is_truncated_per_run() {
    let dir = TempDir::new().unwrap();
    let log = dir.path().join("run.log");
    std::fs::write(&log, "stale\n").unwrap();
    sh_proc()
        .run_streamed(&sh("echo fresh", 5), &Output::Log(log.clone()))
        .unwrap();
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "fresh\n");
}

#[test]
fn non_zero_exit_carries_stderr_tail() {
    let r = sh_proc().run_streamed(&sh("echo boom >&2; exit 3", 5), &Output::Inherit);
    match r {
        Err(ProcError::NonZero {
            code,
            stdout,
            stderr_tail,
        }) => {
            assert_eq!(code, 3);
            assert!(stdout.is_empty());
            assert_eq!(stderr_tail, "boom\n");
        }
        other => unreachable!("expected NonZero, got {other:?}"),
    }
}

#[test]
fn non_zero_exit_still_streams_stderr_to_log() {
    let dir = TempDir::new().unwrap();
    let log = dir.path().join("run.log");
    let r = sh_proc().run_streamed(
        &sh("echo out; echo boom >&2; exit 2", 5),
        &Output::Log(log.clone()),
    );
    assert!(
        matches!(r, Err(ProcError::NonZero { code: 2, .. })),
        "{r:?}"
    );
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("out") && text.contains("boom"), "{text}");
}

#[test]
fn stderr_tail_is_bounded_to_the_last_bytes() {
    let body = "i=0; while [ $i -lt 5000 ]; do echo line$i >&2; i=$((i+1)); done; exit 1";
    let r = sh_proc().run_streamed(&sh(body, 10), &Output::Log("/dev/null".into()));
    match r {
        Err(ProcError::NonZero { stderr_tail, .. }) => {
            assert!(stderr_tail.len() <= 2048, "{}", stderr_tail.len());
            assert!(stderr_tail.ends_with("line4999\n"), "{stderr_tail}");
        }
        other => unreachable!("expected NonZero, got {other:?}"),
    }
}

#[test]
fn zero_exit_is_ok_unit() {
    let r = sh_proc().run_streamed(&sh("echo hi >&2", 5), &Output::Log("/dev/null".into()));
    assert!(r.is_ok(), "{r:?}");
}

#[test]
fn signal_death_maps_to_signalled() {
    let r = sh_proc().run_streamed(&sh("kill -9 $$", 5), &Output::Inherit);
    assert!(matches!(r, Err(ProcError::Signalled(9))), "{r:?}");
}

#[test]
fn unwritable_log_is_spawn_error() {
    let r = sh_proc().run_streamed(
        &sh("true", 5),
        &Output::Log("/nonexistent-dir/run.log".into()),
    );
    assert!(matches!(r, Err(ProcError::Spawn(_))), "{r:?}");
}

#[test]
fn deadline_stops_streamed_child() {
    let t = Instant::now();
    let r = sh_proc().run_streamed(&sh("sleep 30", 1), &Output::Inherit);
    assert!(matches!(r, Err(ProcError::Timeout { .. })), "{r:?}");
    assert!(
        t.elapsed() < Duration::from_millis(3500),
        "{:?}",
        t.elapsed()
    );
}

#[test]
fn interrupt_flag_stops_child_group_with_term() {
    let dir = TempDir::new().unwrap();
    let ready = dir.path().join("ready");
    let pidfile = dir.path().join("grandchild");
    let body = format!(
        "sleep 30 &\necho $! > {}\ntouch {}\nwait",
        pidfile.display(),
        ready.display()
    );
    let flag = Arc::new(AtomicBool::new(false));
    let t = Instant::now();
    let r = std::thread::scope(|s| {
        s.spawn(|| {
            wait_for(&ready);
            flag.store(true, Ordering::Relaxed);
        });
        sh_proc().stream(&sh(&body, 30), &Output::Inherit, &flag)
    });
    assert!(matches!(r, Err(ProcError::Interrupted)), "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    let pid = std::fs::read_to_string(&pidfile).unwrap();
    let stat = Path::new("/proc").join(pid.trim()).join("stat");
    let mut state = String::new();
    for _ in 0..100 {
        state = std::fs::read_to_string(&stat).unwrap_or_default();
        if state.is_empty() || state.contains(") Z") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        state.is_empty() || state.contains(") Z"),
        "grandchild alive: {state}"
    );
}

#[test]
fn child_ignoring_term_is_killed_after_grace() {
    let dir = TempDir::new().unwrap();
    let ready = dir.path().join("ready");
    let body = format!(
        "trap '' TERM\ntouch {}\nwhile :; do sleep 1; done",
        ready.display()
    );
    let flag = Arc::new(AtomicBool::new(false));
    let t = Instant::now();
    let r = std::thread::scope(|s| {
        s.spawn(|| {
            wait_for(&ready);
            flag.store(true, Ordering::Relaxed);
        });
        sh_proc().stream(&sh(&body, 30), &Output::Inherit, &flag)
    });
    assert!(matches!(r, Err(ProcError::Interrupted)), "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

/// Body of the child process run by [`sigterm_sent_to_moor_is_forwarded_as_interrupt`].
/// Signals the whole process, so it must not share one with other tests.
#[test]
#[ignore = "runs only as the child of another test"]
fn forwarded_sigterm_interrupts_stream() {
    let dir = TempDir::new().unwrap();
    let ready = dir.path().join("ready");
    let body = format!("touch {}\nsleep 30", ready.display());
    let r = std::thread::scope(|s| {
        s.spawn(|| {
            wait_for(&ready);
            kill_process(getpid(), Signal::TERM).unwrap();
        });
        sh_proc().run_streamed(&sh(&body, 30), &Output::Inherit)
    });
    assert!(matches!(r, Err(ProcError::Interrupted)), "{r:?}");
}

#[test]
fn sigterm_sent_to_moor_is_forwarded_as_interrupt() {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "adapters::proc::stream::tests::forwarded_sigterm_interrupts_stream",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "{status:?}");
}

/// Body of the child process run by [`sigterm_after_streaming_is_default_action`].
#[test]
#[ignore = "runs only as the child of another test"]
fn terminated_after_streaming() {
    sh_proc()
        .run_streamed(&sh("true", 5), &Output::Inherit)
        .unwrap();
    kill_process(getpid(), Signal::TERM).unwrap();
    std::thread::sleep(Duration::from_secs(5));
}

#[test]
fn sigterm_after_streaming_is_default_action() {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "adapters::proc::stream::tests::terminated_after_streaming",
        ])
        .status()
        .unwrap();
    assert_eq!(status.signal(), Some(15), "{status:?}");
}

#[test]
fn idle_tracks_overlapping_streams() {
    let sig = Signals::new();
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| {
                for _ in 0..2000 {
                    let _a = sig.enter();
                    let _b = sig.enter();
                    assert!(!sig.idle.load(Ordering::SeqCst));
                }
            });
        }
    });
    assert!(sig.idle.load(Ordering::SeqCst));
    let _a = sig.enter();
    assert!(!sig.idle.load(Ordering::SeqCst));
}

#[test]
fn stderr_tail_survives_a_lingering_pipe_holder() {
    let dir = TempDir::new().unwrap();
    let log = dir.path().join("run.log");
    let t = Instant::now();
    let r = sh_proc().run_streamed(
        &sh("sleep 3 & echo 'remote command exited 3' >&2; exit 3", 10),
        &Output::Log(log),
    );
    match r {
        Err(ProcError::NonZero {
            code, stderr_tail, ..
        }) => {
            assert_eq!(code, 3);
            assert!(
                stderr_tail.contains("remote command exited 3"),
                "{stderr_tail:?}"
            );
        }
        other => unreachable!("expected NonZero, got {other:?}"),
    }
    assert!(
        t.elapsed() < Duration::from_millis(2500),
        "{:?}",
        t.elapsed()
    );
}

/// Collects what is written so a test can read it back.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn inherit_sends_child_stderr_to_the_stderr_writer_and_tail() {
    let cap = Capture::default();
    let flag = Arc::new(AtomicBool::new(false));
    let r = sh_proc().stream_to(
        &sh("echo live >&2; exit 4", 5),
        &Output::Inherit,
        &flag,
        Box::new(cap.clone()),
    );
    match r {
        Err(ProcError::NonZero {
            code, stderr_tail, ..
        }) => {
            assert_eq!(code, 4);
            assert_eq!(stderr_tail, "live\n");
        }
        other => unreachable!("expected NonZero, got {other:?}"),
    }
    assert_eq!(cap.0.lock().unwrap().as_slice(), b"live\n");
}

fn gate() -> StderrGate {
    StderrGate {
        begin: "moor-b".into(),
        end: "moor-e".into(),
    }
}

#[test]
fn gate_forwards_only_lines_between_the_markers() {
    let script = "echo 'running on host printf moor-b ...' >&2; echo moor-b >&2; \
                  echo hello >&2; echo moor-e >&2; echo 'run summary' >&2";
    let cap = Capture::default();
    let flag = Arc::new(AtomicBool::new(false));
    let spec = sh(script, 5).stderr_gate(gate());
    sh_proc()
        .stream_to(&spec, &Output::Inherit, &flag, Box::new(cap.clone()))
        .unwrap();
    let shown = String::from_utf8(cap.0.lock().unwrap().clone()).unwrap();
    assert_eq!(shown, "hello\n", "stderr shown to the user");
}

#[test]
fn gate_handles_crlf_markers_and_an_unterminated_last_line() {
    let script = "printf 'moor-b\\r\\none\\r\\ntwo' >&2";
    let cap = Capture::default();
    let flag = Arc::new(AtomicBool::new(false));
    let spec = sh(script, 5).stderr_gate(gate());
    sh_proc()
        .stream_to(&spec, &Output::Inherit, &flag, Box::new(cap.clone()))
        .unwrap();
    assert_eq!(cap.0.lock().unwrap().as_slice(), b"one\r\ntwo");
}

#[test]
fn gate_never_opened_shows_nothing_but_the_error_keeps_the_tail() {
    let cap = Capture::default();
    let flag = Arc::new(AtomicBool::new(false));
    let spec = sh("echo 'sync failed' >&2; exit 5", 5).stderr_gate(gate());
    let r = sh_proc().stream_to(&spec, &Output::Inherit, &flag, Box::new(cap.clone()));
    match r {
        Err(ProcError::NonZero {
            code, stderr_tail, ..
        }) => {
            assert_eq!(code, 5);
            assert_eq!(stderr_tail, "sync failed\n");
        }
        other => unreachable!("expected NonZero, got {other:?}"),
    }
    assert!(cap.0.lock().unwrap().is_empty());
}

#[test]
fn gate_tail_keeps_marker_and_filtered_lines() {
    let script = "echo moor-b >&2; echo hello >&2; echo moor-e >&2; echo after >&2; exit 1";
    let cap = Capture::default();
    let flag = Arc::new(AtomicBool::new(false));
    let spec = sh(script, 5).stderr_gate(gate());
    let r = sh_proc().stream_to(&spec, &Output::Inherit, &flag, Box::new(cap.clone()));
    match r {
        Err(ProcError::NonZero { stderr_tail, .. }) => {
            assert_eq!(stderr_tail, "moor-b\nhello\nmoor-e\nafter\n");
        }
        other => unreachable!("expected NonZero, got {other:?}"),
    }
    assert_eq!(cap.0.lock().unwrap().as_slice(), b"hello\n");
}

#[test]
fn gate_cuts_a_line_without_newline_at_the_cap() {
    let mut open = super::Gate::new(gate());
    assert!(open.feed(b"moor-b\n").is_empty());
    assert_eq!(open.feed(&[b'x'; 8192]).len(), 8192, "flushed at the cap");
    assert!(open.line.is_empty());
    let mut closed = super::Gate::new(gate());
    assert!(closed.feed(&vec![b'x'; 20000]).is_empty());
    assert!(closed.line.len() < 8192, "dropped, not buffered");
}
