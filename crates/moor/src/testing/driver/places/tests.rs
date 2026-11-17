//! Each Driver op hands the fake the place its caller named.

use std::path::Path;
use std::time::Duration;

use moor_core::name::BoxName;
use moor_core::seam::driver::{AcquireReq, BoxStep, Driver, ExecSpec, Place};
use moor_core::seam::proc::{Deadline, Pipes};
use moor_core::{BoxHandle, BoxId, BoxPath, ProviderId};

use super::FakeDriver;

const ALL: [Place; 2] = [Place::Agent, Place::Work];
const DEADLINE: Deadline = Deadline(Duration::from_secs(5));

fn boxed() -> (FakeDriver, BoxHandle) {
    let fake = FakeDriver::new();
    let req = AcquireReq {
        pool_machine: None,
        provider: ProviderId::new("fake"),
        box_id: BoxId::new(1),
        name: BoxName::parse_allow_bare("t1").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
        cache: moor_core::cache::CachePlan::none(),
    };
    let handle = fake.acquire(&req).unwrap().handle;
    (fake, handle)
}

fn path() -> BoxPath {
    BoxPath::under("/work/cbx_1/repo", "f").unwrap()
}

#[test]
fn exec_records_the_specs_place() {
    for place in ALL {
        let (fake, h) = boxed();
        let spec = ExecSpec::new(place, vec!["true".to_owned()]);
        assert!(fake.exec(&h, &spec, DEADLINE).is_ok());
        assert_eq!(fake.places("exec"), [place]);
    }
}

#[test]
fn exec_piped_records_the_specs_place() {
    for place in ALL {
        let (fake, h) = boxed();
        let spec = ExecSpec::new(place, vec!["true".to_owned()]);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let pipes = Pipes {
            stdin: Box::new(std::io::empty()),
            stdout: &mut out,
            stderr: &mut err,
            stop: &|| false,
        };
        assert!(fake.exec_piped(&h, &spec, pipes).is_ok());
        assert_eq!(fake.places("exec_piped"), [place]);
    }
}

#[test]
fn launch_run_records_the_specs_place() {
    for place in ALL {
        let (fake, h) = boxed();
        let spec = ExecSpec::new(place, vec!["true".to_owned()]);
        let _ = fake.launch_run(&h, 1, &spec);
        assert_eq!(fake.places("launch_run"), [place]);
    }
}

#[test]
fn peek_file_records_its_place() {
    for place in ALL {
        let (fake, h) = boxed();
        let _ = fake.peek_file(&h, place, &path());
        assert_eq!(fake.places("peek_file"), [place]);
    }
}

#[test]
fn put_file_records_its_place() {
    for place in ALL {
        let (fake, h) = boxed();
        assert!(fake.put_file(&h, place, &path(), b"x", 0o600).is_ok());
        assert_eq!(fake.places("put_file"), [place]);
    }
}

#[test]
fn copy_out_records_its_place() {
    for place in ALL {
        let (fake, h) = boxed();
        let _ = fake.copy_out(&h, place, &path(), Path::new("/nonexistent/to"));
        assert_eq!(fake.places("copy_out"), [place]);
    }
}

#[test]
fn connect_argv_records_its_place() {
    for place in ALL {
        let (fake, h) = boxed();
        assert!(fake.connect_argv(&h, place).is_ok());
        assert_eq!(fake.places("connect_argv"), [place]);
    }
}

#[test]
fn batch_records_its_place_and_runs_every_step_there() {
    for place in ALL {
        let (fake, h) = boxed();
        let steps = [
            BoxStep::Put {
                path: path(),
                bytes: b"x".to_vec(),
                mode: 0o600,
            },
            BoxStep::Run("true".to_owned()),
            BoxStep::Peek(path()),
        ];
        let done = fake.batch(&h, &[], place, &steps, DEADLINE);
        assert_eq!(done.len(), 3);
        assert_eq!(fake.places("batch"), [place]);
        assert_eq!(fake.places("put_file"), [place]);
        assert_eq!(fake.places("exec"), [place]);
        assert_eq!(fake.places("peek_file"), [place]);
    }
}
