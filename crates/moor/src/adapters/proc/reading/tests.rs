//! Tests for `SystemProc::run_reading` using real `sh`.
#![expect(
    clippy::disallowed_methods,
    reason = "tests time real elapsed durations"
)]

use std::time::{Duration, Instant};

use moor_core::seam::proc::{Deadline, Proc, ProcError, ProcSpec, Tool};

use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::env::Env;

fn sh(script: &str, millis: u64) -> ProcSpec {
    let mut spec = ProcSpec::new(Tool::Sh, Deadline(Duration::from_millis(millis)));
    spec.args = vec!["-c".into(), script.into()];
    spec
}

fn proc() -> SystemProc {
    SystemProc::new(
        &Env::from_pairs(Vec::<(String, String)>::new()),
        &ToolPaths::default(),
    )
}

#[test]
fn binary_stdout_arrives_byte_exact() {
    let mut got = Vec::new();
    let r = proc().run_reading(&sh(r"printf '\377\000\200'", 5000), &mut |out| {
        out.read_to_end(&mut got).unwrap();
    });
    r.unwrap();
    assert_eq!(got, [0xff, 0x00, 0x80]);
}

#[test]
fn reader_may_stop_early_without_deadlock() {
    let mut first = [0_u8; 4];
    let start = Instant::now();
    proc()
        .run_reading(&sh("head -c 20000000 /dev/zero", 10_000), &mut |out| {
            out.read_exact(&mut first).unwrap();
        })
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(first, [0; 4]);
}

#[test]
fn non_zero_exit_carries_stderr_and_no_stdout() {
    let err = proc()
        .run_reading(&sh("echo out; echo boom >&2; exit 3", 5000), &mut |out| {
            drop(out.read_to_end(&mut Vec::new()));
        })
        .unwrap_err();
    let ProcError::NonZero {
        code,
        stdout,
        stderr_tail,
    } = err
    else {
        unreachable!("expected NonZero, got {err:?}")
    };
    assert_eq!(code, 3);
    assert!(stdout.is_empty());
    assert!(stderr_tail.contains("boom"), "{stderr_tail}");
}

#[test]
fn deadline_kills_the_child_and_times_out() {
    let start = Instant::now();
    let err = proc()
        .run_reading(&sh("sleep 30", 300), &mut |out| {
            drop(out.read_to_end(&mut Vec::new()));
        })
        .unwrap_err();
    assert!(matches!(err, ProcError::Timeout { .. }), "{err:?}");
    assert!(start.elapsed() < Duration::from_secs(5));
}
