use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use moor_core::seam::proc::Pipes;
use moor_core::view::ExecView;
use moor_core::{BoxName, ErrorKind, ExecReq};
use serde_json::json;

use crate::ops::run_fixture::{ready_with, Fx};
use crate::testing::driver::FakeDriver;
use crate::Moor;

/// `exec` of `argv` in Box `name`.
fn req(name: &str, argv: &[&str]) -> ExecReq {
    ExecReq {
        target: BoxName::parse_allow_bare(name).unwrap().into(),
        argv: argv.iter().map(|s| (*s).to_owned()).collect(),
        timeout_secs: None,
    }
}

/// Runs `req` against `f` with no stdin; the view and the command's stdout.
fn exec_quiet(f: &Fx, req: &ExecReq) -> (ExecView, Vec<u8>) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let pipes = Pipes {
        stdin: Box::new(std::io::empty()),
        stdout: &mut out,
        stderr: &mut err,
        stop: &|| false,
    };
    let view = crate::ops::exec::run(&f.cx, req, pipes).unwrap();
    (view, out)
}

/// Like [`exec_quiet`], moving the fixture's clock a second on every 50 ms of real time, so a
/// timeout passes without waiting it out; the view.
fn exec_while_time_passes(f: &Fx, req: &ExecReq) -> ExecView {
    let (done, clock) = (AtomicBool::new(false), f.cx.clock());
    std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(50));
                clock.sleep(Duration::from_secs(1));
            }
        });
        let (view, _) = exec_quiet(f, req);
        done.store(true, Ordering::SeqCst);
        view
    })
}

/// The payloads of Box `f`'s `exec_ran` events, oldest first.
fn exec_ran(f: &Fx) -> Vec<serde_json::Value> {
    let mut rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.sort_by_key(|e| e.id);
    rows.into_iter()
        .filter(|e| e.kind == "exec_ran")
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_command_past_its_timeout_is_killed_and_said_to_have_timed_out() {
    let f = ready_with(FakeDriver::new());
    let mut slow = req("t", &["sleep", "30"]);
    slow.timeout_secs = Some(1);
    let view = exec_while_time_passes(&f, &slow);
    assert_eq!(view.exit, None);
    assert!(view.timed_out);
}

#[test]
fn a_command_that_ends_in_time_is_not_timed_out() {
    let f = ready_with(FakeDriver::new());
    let mut quick = req("t", &["echo", "hi"]);
    quick.timeout_secs = Some(30);
    let (view, out) = exec_quiet(&f, &quick);
    assert_eq!(view.exit, Some(0));
    assert!(!view.timed_out);
    assert_eq!(out, b"hi\n");
}

#[test]
fn each_exec_leaves_an_exec_ran_event_with_the_command_and_how_it_ended() {
    let f = ready_with(FakeDriver::new());
    exec_quiet(&f, &req("t", &["sh -c 'exit 3'"]));
    let mut slow = req("t", &["sleep", "30"]);
    slow.timeout_secs = Some(1);
    exec_while_time_passes(&f, &slow);
    assert_eq!(
        exec_ran(&f),
        [
            json!({"command": "sh -c 'exit 3'", "exit": 3, "timed_out": false}),
            json!({"command": "'sleep' '30'", "exit": null, "timed_out": true}),
        ]
    );
}

#[test]
fn the_command_runs_in_the_box_env_with_its_stdio_on_the_pipes() {
    let f = ready_with(FakeDriver::new());
    let driver = f.driver.clone();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let pipes = Pipes {
        stdin: Box::new(Cursor::new(b"hi\n".to_vec())),
        stdout: &mut out,
        stderr: &mut err,
        stop: &|| false,
    };
    let view = Moor::from_cx(f.cx)
        .exec(&req("t", &["cat"]), pipes)
        .unwrap();
    assert_eq!(view.exit, Some(0));
    assert_eq!(out, b"hi\n");
    let execs = driver.execs();
    assert_eq!(execs.len(), 1, "{execs:?}");
    let spec = &execs.first().unwrap().1;
    assert_eq!(
        spec.argv.get(..2),
        Some(&["sh".to_owned(), "-c".to_owned()][..])
    );
    assert!(spec
        .argv
        .get(2)
        .is_some_and(|prelude| prelude.contains(".moor-proxy")));
    assert_eq!(spec.argv.last().map(String::as_str), Some("cat"));
}

#[test]
fn a_box_that_is_not_open_is_refused_before_anything_runs() {
    let f = ready_with(FakeDriver::new());
    let driver = f.driver.clone();
    let mut sink = Vec::new();
    let mut err = Vec::new();
    let pipes = Pipes {
        stdin: Box::new(std::io::empty()),
        stdout: &mut sink,
        stderr: &mut err,
        stop: &|| false,
    };
    let e = Moor::from_cx(f.cx)
        .exec(&req("nope", &["true"]), pipes)
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().contains("no open Box nope"), "{e:?}");
    assert!(driver.execs().is_empty());
}

#[test]
fn an_empty_command_is_a_usage_error() {
    let f = ready_with(FakeDriver::new());
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let pipes = Pipes {
        stdin: Box::new(std::io::empty()),
        stdout: &mut out,
        stderr: &mut err,
        stop: &|| false,
    };
    let e = Moor::from_cx(f.cx).exec(&req("t", &[]), pipes).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Usage);
}
