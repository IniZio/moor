//! Tests for long-lived children using shell scripts.
#![expect(
    clippy::disallowed_methods,
    reason = "tests poll real elapsed time for process exit"
)]

use std::time::{Duration, Instant};

use moor_core::seam::proc::{Deadline, FirstLineError, Proc, ProcSpec, Tool};
use tempfile::TempDir;

use super::SystemProc;
use crate::adapters::proc::ToolPaths;
use crate::env::Env;

fn sys() -> SystemProc {
    SystemProc::new(
        &Env::from_pairs([("PATH", "/usr/bin:/bin")]),
        &ToolPaths::default(),
    )
}

fn sh(body: &str) -> ProcSpec {
    ProcSpec::new(Tool::Sh, Deadline(Duration::from_secs(5)))
        .arg("-c")
        .arg(body)
}

fn secs(n: u64) -> Deadline {
    Deadline(Duration::from_secs(n))
}

/// True when the process is absent or only a zombie awaiting its reaper.
fn dead(p: &SystemProc, pid: u32) -> bool {
    if !p.alive(pid) {
        return true;
    }
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |s| {
        s.rsplit(") ").next().is_some_and(|r| r.starts_with('Z'))
    })
}

fn wait_dead(p: &SystemProc, pid: u32) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if dead(p, pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn ready_line_is_cached() {
    let p = sys();
    let mut c = p.spawn_child(&sh("echo ready; sleep 300")).unwrap();
    assert_eq!(c.first_line(secs(5)).unwrap(), "ready");
    let start = Instant::now();
    assert_eq!(c.first_line(Deadline(Duration::ZERO)).unwrap(), "ready");
    assert!(start.elapsed() < Duration::from_millis(100));
}

#[test]
fn timeout_keeps_child_running() {
    let p = sys();
    let mut c = p.spawn_child(&sh("sleep 300")).unwrap();
    let after = Duration::from_millis(200);
    let err = c.first_line(Deadline(after)).unwrap_err();
    assert!(
        matches!(err, FirstLineError::Timeout { tool: Tool::Sh, after: a } if a == after),
        "{err:?}"
    );
    assert!(p.alive(c.pid()));
}

#[test]
fn early_exit_reports_status_and_stderr() {
    let p = sys();
    let mut c = p.spawn_child(&sh("echo boom >&2; exit 3")).unwrap();
    let err = c.first_line(secs(5)).unwrap_err();
    assert!(
        matches!(&err, FirstLineError::Exited { code: Some(3), signal: None, stderr_tail }
            if stderr_tail.contains("boom")),
        "{err:?}"
    );
}

#[test]
fn partial_line_is_not_a_line() {
    let p = sys();
    let mut c = p.spawn_child(&sh("printf partial")).unwrap();
    assert!(matches!(
        c.first_line(secs(5)),
        Err(FirstLineError::Exited { code: Some(0), .. })
    ));
}

#[test]
fn drop_kills_whole_group() {
    let p = sys();
    let mut c = p.spawn_child(&sh("sleep 300 & echo $!; wait")).unwrap();
    let leader = c.pid();
    let grandchild: u32 = c.first_line(secs(5)).unwrap().parse().unwrap();
    assert!(p.alive(grandchild));
    drop(c);
    assert!(wait_dead(&p, grandchild), "grandchild survived drop");
    assert!(wait_dead(&p, leader), "leader survived drop");
}

#[test]
fn drop_kills_leader_ignoring_term() {
    let p = sys();
    let mut c = p
        .spawn_child(&sh("trap '' TERM; echo ready; while :; do sleep 1; done"))
        .unwrap();
    let leader = c.pid();
    assert_eq!(c.first_line(secs(5)).unwrap(), "ready");
    drop(c);
    assert!(wait_dead(&p, leader), "leader survived drop");
}

#[test]
fn cwd_and_env_reach_child() {
    let dir = TempDir::new().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let spec = sh(r#"echo "$PWD|$FOO"; sleep 300"#)
        .cwd(&cwd)
        .env("FOO", "bar");
    let p = sys();
    let mut c = p.spawn_child(&spec).unwrap();
    assert_eq!(
        c.first_line(secs(5)).unwrap(),
        format!("{}|bar", cwd.display())
    );
}
