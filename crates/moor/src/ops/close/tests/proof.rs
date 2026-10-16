use std::fs;
use std::path::PathBuf;

use moor_core::seam::driver::DriverError;
use moor_core::ErrorKind;

use super::{fx, fx_with, req, run, Fx};
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

const SHOT: &[u8] = &[0x89, b'P', b'N', b'G', 0, 1, 2, 255];
const LOG: &[u8] = b"line one\nline two\n";

/// Writes two proof copies for task `t` under the state directory.
fn seed(f: &Fx) -> (PathBuf, PathBuf) {
    let root = f.dir.path().join("proof").join("t");
    fs::create_dir_all(root.join("sub")).unwrap();
    let shot = root.join("shot.png");
    let log = root.join("sub/log.txt");
    fs::write(&shot, SHOT).unwrap();
    fs::write(&log, LOG).unwrap();
    (shot, log)
}

fn assert_kept(shot: &PathBuf, log: &PathBuf) {
    assert_eq!(fs::read(shot).unwrap(), SHOT);
    assert_eq!(fs::read(log).unwrap(), LOG);
}

#[test]
fn close_keeps_the_proof_copies() {
    let f = fx();
    let (shot, log) = seed(&f);
    run(&f.cx, &req(false)).unwrap();
    assert_kept(&shot, &log);
}

#[test]
fn a_refused_close_keeps_the_proof_copies() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 2);
    let (shot, log) = seed(&f);
    assert_eq!(
        run(&f.cx, &req(false)).unwrap_err().kind,
        ErrorKind::Refused
    );
    assert_kept(&shot, &log);
}

#[test]
fn a_failed_release_keeps_the_proof_copies() {
    let f = fx_with(
        FakeDriver::new().fail_release_always(DriverError::Failed {
            summary: "boom".into(),
        }),
        FakeSurface::new("/s"),
        0,
    );
    let (shot, log) = seed(&f);
    assert_eq!(run(&f.cx, &req(false)).unwrap_err().kind, ErrorKind::Failed);
    assert_kept(&shot, &log);
}
