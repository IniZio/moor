//! Tests for streamed children using `sh -c`.
#![expect(
    clippy::disallowed_methods,
    reason = "tests time real elapsed durations"
)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use moor_core::seam::proc::{Deadline, Output, Proc, ProcError, ProcSpec, Tool};
use rustix::process::{getpid, kill_process, Signal};
use tempfile::TempDir;

use super::super::{SystemProc, ToolPaths};
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
    let code = sh_proc()
        .run_streamed(&sh("echo out; echo err >&2", 5), &Output::Log(log.clone()))
        .unwrap();
    assert_eq!(code, 0);
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
fn exit_codes_pass_through() {
    let p = sh_proc();
    for want in [1, 3, 125] {
        let got = p
            .run_streamed(&sh(&format!("exit {want}"), 5), &Output::Inherit)
            .unwrap();
        assert_eq!(got, want);
    }
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
    let flag = AtomicBool::new(false);
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
    let flag = AtomicBool::new(false);
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

#[test]
fn sigterm_sent_to_moor_is_forwarded_as_interrupt() {
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
