//! Tests for `SystemProc` using small shell scripts as fake tools.
#![expect(clippy::print_stderr, reason = "timing evidence under --nocapture")]
#![expect(
    clippy::disallowed_methods,
    reason = "tests time real elapsed durations"
)]

use std::path::Path;
use std::time::{Duration, Instant};

use moor_core::seam::proc::{Deadline, Proc, ProcError, ProcSpec, Stdin, Tool};
use tempfile::TempDir;

use super::{SystemProc, ToolPaths};
use crate::env::Env;

fn script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    moor_testkit::write_executable(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    path.to_string_lossy().into_owned()
}

fn proc_with(env: Vec<(&str, String)>) -> SystemProc {
    let env = Env::from_pairs(env.into_iter().map(|(k, v)| (k.to_owned(), v)));
    SystemProc::new(&env, &ToolPaths::default())
}

fn spec(secs: u64) -> ProcSpec {
    ProcSpec::new(Tool::Git, Deadline(Duration::from_secs(secs)))
}

#[test]
fn only_base_env_and_extra_reach_child() {
    let dir = TempDir::new().unwrap();
    let bin = script(dir.path(), "git", r#"echo "$GH_TOKEN|$HOME|$FOO|$LANG""#);
    let p = proc_with(vec![
        ("MOOR_GIT_BIN", bin),
        ("GH_TOKEN", "leak".into()),
        ("HOME", "/h".into()),
    ]);
    let out = p.run(&spec(5).env("FOO", "bar")).unwrap();
    assert_eq!(out.stdout.trim(), "|/h|bar|");
}

#[test]
fn only_crabbox_gets_container_env() {
    let dir = TempDir::new().unwrap();
    let body = r#"echo "$CRABBOX_FOO|$DOCKER_HOST|$GH_TOKEN|$GITHUB_TOKEN|$ANTHROPIC_API_KEY|$CLAUDE_CODE_OAUTH_TOKEN""#;
    let crabbox = script(dir.path(), "crabbox", body);
    let git = script(dir.path(), "git", body);
    let p = proc_with(vec![
        ("MOOR_CRABBOX_BIN", crabbox),
        ("MOOR_GIT_BIN", git),
        ("CRABBOX_FOO", "c".into()),
        ("DOCKER_HOST", "unix:///d.sock".into()),
        ("GH_TOKEN", "leak".into()),
        ("GITHUB_TOKEN", "leak".into()),
        ("ANTHROPIC_API_KEY", "leak".into()),
        ("CLAUDE_CODE_OAUTH_TOKEN", "leak".into()),
    ]);
    let crab = ProcSpec::new(Tool::Crabbox, Deadline(Duration::from_secs(5)));
    assert_eq!(p.run(&crab).unwrap().stdout.trim(), "c|unix:///d.sock||||");
    assert_eq!(p.run(&spec(5)).unwrap().stdout.trim(), "|||||");
}

#[test]
fn bin_var_beats_config_beats_path() {
    let dir = TempDir::new().unwrap();
    let a = script(dir.path(), "a", "echo env");
    let b = script(dir.path(), "b", "echo cfg");
    let paths = ToolPaths {
        git: Some(b.into()),
        ..ToolPaths::default()
    };
    let env = Env::from_pairs([("MOOR_GIT_BIN", a)]);
    let out = SystemProc::new(&env, &paths).run(&spec(5)).unwrap();
    assert_eq!(out.stdout.trim(), "env");
    let out = SystemProc::new(&Env::from_pairs([("X", "y")]), &paths)
        .run(&spec(5))
        .unwrap();
    assert_eq!(out.stdout.trim(), "cfg");
}

#[test]
fn deadline_kills_child_and_grandchild() {
    let dir = TempDir::new().unwrap();
    let pidfile = dir.path().join("pid");
    let body = format!("sleep 30 &\necho $! > {}\nwait", pidfile.display());
    let bin = script(dir.path(), "git", &body);
    let p = proc_with(vec![("MOOR_GIT_BIN", bin)]);
    let start = Instant::now();
    let err = p
        .run(&ProcSpec::new(
            Tool::Git,
            Deadline(Duration::from_millis(500)),
        ))
        .unwrap_err();
    assert!(matches!(err, ProcError::Timeout { .. }), "{err:?}");
    assert!(start.elapsed() < Duration::from_secs(10));
    let pid = std::fs::read_to_string(&pidfile).unwrap();
    let alive = Path::new("/proc").join(pid.trim()).join("stat");
    let mut state = String::new();
    for _ in 0..100 {
        state = std::fs::read_to_string(&alive).unwrap_or_default();
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
fn nonzero_exit_keeps_stdout_and_stderr_tail() {
    let dir = TempDir::new().unwrap();
    let bin = script(dir.path(), "git", "echo out; echo err >&2; exit 3");
    let p = proc_with(vec![("MOOR_GIT_BIN", bin)]);
    let err = p.run(&spec(5)).unwrap_err();
    let ProcError::NonZero {
        code,
        stdout,
        stderr_tail,
    } = err
    else {
        unreachable!("expected NonZero")
    };
    assert_eq!(code, 3);
    assert_eq!(stdout.trim(), "out");
    assert_eq!(stderr_tail.trim(), "err");
}

#[test]
fn signal_death_maps_to_signalled() {
    let dir = TempDir::new().unwrap();
    let bin = script(dir.path(), "git", "kill -9 $$");
    let p = proc_with(vec![("MOOR_GIT_BIN", bin)]);
    assert!(matches!(p.run(&spec(5)), Err(ProcError::Signalled(9))));
}

#[test]
fn missing_program_is_spawn_error() {
    let p = proc_with(vec![("MOOR_GIT_BIN", "/nonexistent/moor-git".into())]);
    assert!(matches!(p.run(&spec(5)), Err(ProcError::Spawn(_))));
}

#[test]
fn null_stdin_is_eof_and_bytes_reach_child() {
    let dir = TempDir::new().unwrap();
    let bin = script(dir.path(), "git", "cat");
    let p = proc_with(vec![("MOOR_GIT_BIN", bin)]);
    assert_eq!(p.run(&spec(5)).unwrap().stdout, "");
    let out = p
        .run(&spec(5).stdin(Stdin::Bytes(b"hello".to_vec())))
        .unwrap();
    assert_eq!(out.stdout, "hello");
}

#[test]
fn detached_child_runs_without_waiting() {
    let dir = TempDir::new().unwrap();
    let marker = dir.path().join("done");
    let bin = script(dir.path(), "git", &format!("touch {}", marker.display()));
    let p = proc_with(vec![("MOOR_GIT_BIN", bin)]);
    p.spawn_detached(&spec(5)).unwrap();
    let start = Instant::now();
    while !marker.exists() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(marker.exists());
}

fn sh(script: &str, secs: u64) -> ProcSpec {
    spec(secs).arg("-c").arg(script)
}

fn sh_proc() -> SystemProc {
    proc_with(vec![("MOOR_GIT_BIN", "/bin/sh".into())])
}

#[test]
fn escaped_grandchild_holding_pipe_does_not_block() {
    let t = Instant::now();
    let out = sh_proc().run(&sh("setsid sleep 8 & echo hi", 2)).unwrap();
    let el = t.elapsed();
    eprintln!("escaped_grandchild elapsed: {el:?}");
    assert_eq!(out.stdout.trim(), "hi");
    assert!(el < Duration::from_secs(3), "{el:?}");
}

#[test]
fn hung_child_times_out_within_grace() {
    let t = Instant::now();
    let err = sh_proc().run(&sh("sleep 30", 1)).unwrap_err();
    let el = t.elapsed();
    eprintln!("hung_child elapsed: {el:?}");
    assert!(matches!(err, ProcError::Timeout { .. }), "{err:?}");
    assert!(el < Duration::from_millis(3500), "{el:?}");
}

#[test]
fn oversized_output_is_capped_and_flagged() {
    let t = Instant::now();
    let out = sh_proc().run(&sh("head -c 5000000 /dev/zero", 10)).unwrap();
    eprintln!("oversized elapsed: {:?}", t.elapsed());
    assert_eq!(out.stdout.len(), super::CAP);
    assert!(out.truncated);
}

#[test]
fn run_secret_returns_raw_non_utf8_bytes() {
    let out = sh_proc()
        .run_secret(&sh("printf '\\377\\376x'", 5))
        .unwrap();
    assert_eq!(out.as_slice(), b"\xff\xfex");
}

#[test]
fn run_secret_failure_carries_no_stdout() {
    let err = sh_proc()
        .run_secret(&sh("echo hunter2; echo err >&2; exit 3", 5))
        .unwrap_err();
    let ProcError::NonZero {
        code,
        stdout,
        stderr_tail,
    } = err
    else {
        unreachable!("expected NonZero")
    };
    assert_eq!(code, 3);
    assert!(stdout.is_empty());
    assert_eq!(stderr_tail.trim(), "err");
}

#[test]
fn alive_sees_own_pid_and_rejects_absent_ones() {
    let p = proc_with(vec![]);
    assert!(p.alive(std::process::id()));
    assert!(p.alive(1));
    assert!(!p.alive(0));
    assert!(!p.alive(u32::MAX));
    assert!(!p.alive(0x7fff_ffff));
}

#[test]
fn crabbox_grace_ignores_the_deadline() {
    assert_eq!(
        super::grace::grace_for(Tool::Crabbox, Duration::from_secs(1)),
        Duration::from_secs(35)
    );
}

#[test]
fn other_tools_grace_is_capped_by_the_deadline() {
    assert_eq!(
        super::grace::grace_for(Tool::Ssh, Duration::from_secs(1)),
        Duration::from_secs(1)
    );
    assert_eq!(
        super::grace::grace_for(Tool::Ssh, Duration::from_secs(60)),
        Duration::from_secs(2)
    );
}
