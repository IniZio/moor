//! Self-tests for the Driver fake.

use std::sync::Arc;
use std::time::Duration;

use moor_core::name::TaskName;
use moor_core::seam::driver::{
    AcquireReq, Driver, DriverError, EgressPlan, ExecOutcome, ExecSpec, Released,
};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath, ProviderId, TaskId};

use super::FakeDriver;

fn req() -> AcquireReq {
    AcquireReq {
        provider: ProviderId::new("fake"),
        task: TaskId::new(1),
        name: TaskName::parse("t1").unwrap(),
        egress: EgressPlan::Open,
        class: None,
        nonce: "n0".to_owned(),
        worktree: None,
    }
}

#[test]
fn shared_arc_sees_calls_and_vanished_box_is_already_gone() {
    let fake = Arc::new(FakeDriver::new());
    let driver: Arc<dyn Driver> = fake.clone();
    let got = driver.acquire(&req());
    assert_eq!(got.as_ref().map(|a| a.handle.as_str()), Ok("box-1"));
    fake.forget(&BoxHandle::new("box-1"));
    assert_eq!(
        driver.release(&BoxHandle::new("box-1")),
        Ok(Released::AlreadyGone)
    );
    assert_eq!(fake.calls(), ["acquire", "release box-1"]);
}

#[test]
fn scripted_acquire_failure_is_one_shot() {
    let fake = FakeDriver::new().fail_acquire(DriverError::Quota("q".into()));
    assert_eq!(fake.acquire(&req()), Err(DriverError::Quota("q".into())));
    assert!(fake.acquire(&req()).is_ok());
}

#[test]
fn planned_handle_is_the_next_acquired_handle() {
    let fake = FakeDriver::new();
    let planned = fake.planned_handle(&req());
    assert_eq!(fake.acquire(&req()).unwrap().handle, planned);
}

fn spec(argv: &[&str]) -> ExecSpec {
    ExecSpec {
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        env: vec![("K".to_owned(), "V".to_owned())],
        cwd: Some(BoxPath::under("/work", "box-1").unwrap()),
    }
}

const DL: Deadline = Deadline(Duration::from_secs(7));

fn fake_with_box() -> (FakeDriver, BoxHandle) {
    let fake = FakeDriver::new();
    let handle = fake.acquire(&req()).unwrap().handle;
    (fake, handle)
}

#[test]
fn exec_records_argv_env_cwd_and_deadline_of_each_call() {
    let (fake, h) = fake_with_box();
    let (a, b) = (spec(&["ls", "-l"]), spec(&["true"]));
    assert_eq!(fake.exec(&h, &a, DL), Ok(ExecOutcome { code: 0 }));
    assert_eq!(fake.exec(&h, &b, DL), Ok(ExecOutcome { code: 0 }));
    assert_eq!(fake.execs(), [(h, a, DL), (BoxHandle::new("box-1"), b, DL)]);
    assert_eq!(fake.calls(), ["acquire", "exec box-1", "exec box-1"]);
}

#[test]
fn scripted_exec_outcomes_are_fifo_then_default_to_success() {
    let (fake, h) = fake_with_box();
    let fake = fake.exec_results([Ok(ExecOutcome { code: 3 })]);
    assert_eq!(
        fake.exec(&h, &spec(&["x"]), DL),
        Ok(ExecOutcome { code: 3 })
    );
    assert_eq!(
        fake.exec(&h, &spec(&["x"]), DL),
        Ok(ExecOutcome { code: 0 })
    );
}

#[test]
fn scripted_exec_error_is_returned() {
    let (fake, h) = fake_with_box();
    let fake = fake.exec_results([Err(DriverError::Quota("q".into()))]);
    assert_eq!(
        fake.exec(&h, &spec(&["x"]), DL),
        Err(DriverError::Quota("q".into()))
    );
}

#[test]
fn exec_on_unknown_handle_is_not_found_but_recorded() {
    let fake = FakeDriver::new();
    let ghost = BoxHandle::new("box-9");
    assert_eq!(
        fake.exec(&ghost, &spec(&["x"]), DL),
        Err(DriverError::NotFound)
    );
    assert_eq!(fake.execs().len(), 1);
    assert_eq!(fake.calls(), ["exec box-9"]);
}
