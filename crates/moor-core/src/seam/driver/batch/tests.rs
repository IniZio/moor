#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use super::*;
use crate::name::{BoxPath, ProviderId};
use crate::seam::driver::{AcquireReq, Acquired, BoxListing, DriverCaps, Released, SleepOutcome};

struct Scripted {
    exec_codes: Vec<i32>,
    next: AtomicUsize,
    peek_fails: bool,
    places: Mutex<Vec<Place>>,
}

impl Scripted {
    fn note(&self, place: Place) {
        if let Ok(mut seen) = self.places.lock() {
            seen.push(place);
        }
    }
}

fn no<T>() -> Result<T, DriverError> {
    Err(DriverError::Unsupported("test"))
}

impl crate::seam::driver::Forwards for Scripted {}

impl Driver for Scripted {
    fn caps(&self, _: &ProviderId) -> Result<DriverCaps, DriverError> {
        no()
    }
    fn acquire(&self, _: &AcquireReq) -> Result<Acquired, DriverError> {
        no()
    }
    fn planned_handle(&self, _: &AcquireReq) -> BoxHandle {
        BoxHandle::new("h")
    }
    fn release(&self, _: &BoxHandle) -> Result<Released, DriverError> {
        no()
    }
    fn list(&self, _: &ProviderId) -> Result<Vec<BoxListing>, DriverError> {
        no()
    }
    fn workdir(&self, _: &BoxHandle) -> Result<BoxPath, DriverError> {
        no()
    }
    fn exec(
        &self,
        _: &BoxHandle,
        spec: &ExecSpec,
        _: Deadline,
    ) -> Result<ExecOutcome, DriverError> {
        self.note(spec.place);
        assert_eq!(spec.argv[..2], ["sh", "-c"]);
        let i = self.next.fetch_add(1, Ordering::SeqCst);
        Ok(ExecOutcome::Exited {
            code: self.exec_codes[i],
        })
    }
    fn copy_out(&self, _: &BoxHandle, _: Place, _: &BoxPath, _: &Path) -> Result<(), DriverError> {
        no()
    }
    fn peek_file(
        &self,
        _: &BoxHandle,
        place: Place,
        _: &BoxPath,
    ) -> Result<Option<String>, DriverError> {
        self.note(place);
        if self.peek_fails {
            Err(DriverError::NotFound)
        } else {
            Ok(None)
        }
    }
    fn put_file(
        &self,
        _: &BoxHandle,
        place: Place,
        _: &BoxPath,
        _: &[u8],
        _: u32,
    ) -> Result<(), DriverError> {
        self.note(place);
        Ok(())
    }
    fn connect_argv(&self, _: &BoxHandle, _: Place) -> Result<Vec<String>, DriverError> {
        no()
    }
    fn sleep(&self, _: &BoxHandle) -> Result<SleepOutcome, DriverError> {
        no()
    }
    fn wake(&self, _: &BoxHandle) -> Result<(), DriverError> {
        no()
    }
}

fn path() -> BoxPath {
    BoxPath::under("/w", "f").unwrap()
}

fn run(d: &Scripted, steps: &[BoxStep]) -> Vec<Result<StepDone, DriverError>> {
    run_in(d, Place::Work, steps)
}

fn run_in(d: &Scripted, place: Place, steps: &[BoxStep]) -> Vec<Result<StepDone, DriverError>> {
    run_each(
        d,
        &BoxHandle::new("h"),
        place,
        steps,
        Deadline(Duration::from_secs(1)),
    )
}

#[test]
fn stops_after_nonzero_run() {
    let d = Scripted {
        exec_codes: vec![0, 3, 0],
        next: AtomicUsize::new(0),
        peek_fails: false,
        places: Mutex::default(),
    };
    let steps = [
        BoxStep::Run("a".into()),
        BoxStep::Run("b".into()),
        BoxStep::Peek(path()),
    ];
    let out = run(&d, &steps);
    assert_eq!(out.len(), 2);
    assert!(matches!(
        out[1],
        Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 3 },
            ..
        })
    ));
}

#[test]
fn stops_after_err() {
    let d = Scripted {
        exec_codes: vec![],
        next: AtomicUsize::new(0),
        peek_fails: true,
        places: Mutex::default(),
    };
    let steps = [BoxStep::Peek(path()), BoxStep::Run("a".into())];
    let out = run(&d, &steps);
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0], Err(DriverError::NotFound)));
}

#[test]
fn runs_all_when_clean() {
    let d = Scripted {
        exec_codes: vec![0],
        next: AtomicUsize::new(0),
        peek_fails: false,
        places: Mutex::default(),
    };
    let steps = [
        BoxStep::Put {
            path: path(),
            bytes: vec![1],
            mode: 0o600,
        },
        BoxStep::Run("a".into()),
        BoxStep::Peek(path()),
    ];
    let out = run(&d, &steps);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0], Ok(StepDone::Put));
}

#[test]
fn debug_hides_put_bytes() {
    let s = BoxStep::Put {
        path: path(),
        bytes: b"secret".to_vec(),
        mode: 0o600,
    };
    let text = format!("{s:?}");
    assert!(!text.contains("secret") && text.contains("len: 6"));
}

#[test]
fn every_step_runs_in_the_place_the_batch_was_given() {
    for place in [Place::Agent, Place::Work] {
        let d = Scripted {
            exec_codes: vec![0],
            next: AtomicUsize::new(0),
            peek_fails: false,
            places: Mutex::default(),
        };
        let steps = [
            BoxStep::Put {
                path: path(),
                bytes: vec![1],
                mode: 0o600,
            },
            BoxStep::Run("a".into()),
            BoxStep::Peek(path()),
        ];
        assert_eq!(run_in(&d, place, &steps).len(), 3);
        let seen = d.places.lock().map(|p| p.clone()).unwrap_or_default();
        assert_eq!(seen, [place; 3]);
    }
}
