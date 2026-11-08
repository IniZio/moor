//! Tests for one supervised process's output and its group.
#![allow(clippy::panic, reason = "test assertions")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::{now, supervise, Left, Next, MAX_OUTPUT};

#[test]
fn a_line_with_no_end_is_cut_at_the_cap() {
    let mut cmd = Command::new("sh");
    cmd.args([
        "-c",
        "head -c 3000000 /dev/zero | tr '\\0' a; echo; echo next",
    ]);
    let deadline = now() + Duration::from_secs(30);
    let (lines, _) = supervise(cmd, b"", |lines| {
        let mut seen = Vec::new();
        while let Next::Line(line) = lines.next(deadline) {
            seen.push(line);
        }
        (seen, Left::Ended)
    })
    .unwrap();
    let lens: Vec<usize> = lines.iter().map(String::len).collect();
    assert_eq!(lens, [MAX_OUTPUT]);
}

/// Runs `cmd` until it ends or `limit` passes: its output and whether the limit hit.
fn run(cmd: Command, limit: Duration) -> (Vec<u8>, bool) {
    let deadline = now() + limit;
    let ((out, timed_out), _) = supervise(cmd, b"", |lines| {
        let mut out = Vec::new();
        loop {
            match lines.next(deadline) {
                Next::Line(line) => {
                    out.extend(line.bytes());
                    out.push(b'\n');
                }
                Next::End => return ((out, false), Left::Ended),
                Next::Late => return ((out, true), Left::Stopped),
            }
        }
    })
    .unwrap();
    (out, timed_out)
}

#[test]
fn a_run_past_its_limit_is_killed_with_its_whole_group() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild");
    let mut cmd = Command::new("/bin/sh");
    let script = format!("sleep 60 & echo $! > {}; sleep 30", pid_file.display());
    cmd.args(["-c", &script]);
    let started = now();
    let (_, timed_out) = run(cmd, Duration::from_millis(500));
    assert!(timed_out);
    let took = now().duration_since(started);
    assert!(
        took < Duration::from_secs(5),
        "waited {took:?} for the group"
    );
    assert_gone(&pid_file);
}

/// Waits up to 2 s for the process whose pid is in `pid_file` to be gone or a zombie.
fn assert_gone(pid_file: &Path) {
    let pid = std::fs::read_to_string(pid_file).unwrap();
    let stat = PathBuf::from(format!("/proc/{}/stat", pid.trim()));
    for _ in 0..100 {
        let alive = std::fs::read_to_string(&stat).is_ok_and(|s| !s.contains(") Z "));
        if !alive {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the grandchild outlived the run");
}

#[test]
fn nothing_outlives_a_run_that_ends_on_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild");
    let mut cmd = Command::new("/bin/sh");
    let script = format!(
        "sleep 60 >/dev/null 2>&1 & echo $! > {}; echo done",
        pid_file.display()
    );
    cmd.args(["-c", &script]);
    let (out, timed_out) = run(cmd, Duration::from_secs(30));
    assert!(!timed_out);
    assert_eq!(out, b"done\n");
    assert_gone(&pid_file);
}
