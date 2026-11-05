//! Tests for the per-line progress the setup runner reports.

use std::sync::Mutex;
use std::time::Duration;

use moor_core::domain::decision::ExtraEvent;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::ExecOutcome;

use super::super::lines::Lines;
use super::super::{run_setup as run_setup_with, SetupRun};
use super::{cfg, exits, rig, RUN};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;

fn started(index: u32, total: u32, line: &str) -> ExtraEvent {
    ExtraEvent::SetupLineStarted {
        index,
        total,
        line: line.to_owned(),
    }
}

fn finished(index: u32, total: u32, ok: bool) -> ExtraEvent {
    ExtraEvent::SetupLineFinished {
        index,
        total,
        ms: 0,
        ok,
    }
}

/// Runs `steps` against a Box whose script prints `stdout` and exits with `code`.
fn run(steps: &[&str], stdout: &str, code: i32) -> (SetupRun, Vec<ExtraEvent>) {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_stdout([stdout])
        .exec_results(exits(&[code]));
    let r = rig(d.path(), driver);
    let seen = Mutex::new(Vec::new());
    let got = run_setup_with(
        &r.cx,
        &r.handle,
        ("p", RUN),
        &r.workdir,
        &d.path().join("logs"),
        &cfg(steps, &[]),
        None,
        &|e| seen.lock().unwrap().push(e),
    );
    (got, seen.into_inner().unwrap())
}

#[test]
fn every_line_is_reported_as_it_starts_and_when_it_ends() {
    let (got, events) = run(
        &["a", "b", "c"],
        "moor setup: $ a\nout\nmoor setup: $ b\nmoor setup: $ c\n",
        0,
    );
    assert!(!got.failed());
    assert_eq!(
        events,
        [
            started(1, 3, "a"),
            finished(1, 3, true),
            started(2, 3, "b"),
            finished(2, 3, true),
            started(3, 3, "c"),
            finished(3, 3, true),
        ]
    );
}

#[test]
fn the_script_stops_at_the_first_failed_line_and_later_lines_are_never_reported() {
    let (got, events) = run(
        &["a", "bad", "c"],
        "moor setup: $ a\nmoor setup: $ bad\nmoor setup: step 2 exited 3\n",
        3,
    );
    assert!(got.failed());
    assert_eq!(got.exit(), Some(3));
    assert_eq!(
        events,
        [
            started(1, 3, "a"),
            finished(1, 3, true),
            started(2, 3, "bad"),
            finished(2, 3, false),
        ]
    );
}

#[test]
fn a_script_that_never_announces_a_line_reports_nothing() {
    let (_, events) = run(&["a"], "", 1);
    assert!(events.is_empty());
}

#[test]
fn a_long_or_multiline_line_is_cut_for_the_status_line() {
    let long = "x".repeat(200);
    let (_, events) = run(
        &[long.as_str(), "echo one\necho two"],
        "moor setup: $ x\nmoor setup: $ y\n",
        0,
    );
    let text = |e: &ExtraEvent| match e {
        ExtraEvent::SetupLineStarted { line, .. } => Some(line.clone()),
        _ => None,
    };
    let lines: Vec<String> = events.iter().filter_map(text).collect();
    assert_eq!(
        lines,
        [format!("{}…", "x".repeat(80)), "echo one…".to_owned()]
    );
}

#[test]
fn announcements_split_across_reads_are_still_counted_once() {
    let steps = ["a".to_owned(), "b".to_owned()];
    let mut lines = Lines::new(&steps);
    let t0 = FakeClock::new(Ts::from_millis(0)).instant();
    assert!(lines.feed(b"moor setup", t0).is_empty());
    let first = lines.feed(b": $ a\nnoise\nmoor setup: $", t0);
    assert_eq!(first, [started(1, 2, "a")]);
    let t1 = t0 + Duration::from_secs(12);
    let second = lines.feed(b" b\n", t1);
    assert_eq!(
        second,
        [
            ExtraEvent::SetupLineFinished {
                index: 1,
                total: 2,
                ms: 12_000,
                ok: true,
            },
            started(2, 2, "b"),
        ]
    );
    assert_eq!(
        lines.finish(false, t1),
        [ExtraEvent::SetupLineFinished {
            index: 2,
            total: 2,
            ms: 0,
            ok: false,
        }]
    );
}

#[test]
fn an_attach_again_after_a_cut_reports_each_line_once() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_stdout(["moor setup: $ a\n", "moor setup: $ a\n"])
        .exec_results([Ok(ExecOutcome::Interrupted), Ok(ExecOutcome::Exited { code: 0 })]);
    let r = rig(d.path(), driver);
    let seen = Mutex::new(Vec::new());
    run_setup_with(
        &r.cx,
        &r.handle,
        ("p", RUN),
        &r.workdir,
        &d.path().join("logs"),
        &cfg(&["a"], &[]),
        None,
        &|e| seen.lock().unwrap().push(e),
    );
    assert_eq!(
        seen.into_inner().unwrap(),
        [started(1, 1, "a"), finished(1, 1, true)]
    );
}
