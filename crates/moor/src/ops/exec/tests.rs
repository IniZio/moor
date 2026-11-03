use std::io::Cursor;

use moor_core::seam::proc::Pipes;
use moor_core::{BoxName, ErrorKind, ExecReq};

use crate::ops::run_fixture::ready_with;
use crate::testing::driver::FakeDriver;
use crate::Moor;

/// `exec` of `argv` in Box `name`.
fn req(name: &str, argv: &[&str]) -> ExecReq {
    ExecReq {
        target: BoxName::parse_allow_bare(name).unwrap().into(),
        argv: argv.iter().map(|s| (*s).to_owned()).collect(),
    }
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
