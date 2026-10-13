//! Self-tests for the Driver fake.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::run::SyncMode;
use moor_core::name::TaskName;
use moor_core::seam::driver::{
    AcquireReq, Driver, DriverError, EgressPlan, ExecOutcome, ExecSpec, Released,
};
use moor_core::seam::proc::{Deadline, Output};
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
        sync: SyncMode::Full,
        output: None,
        host_cwd: None,
        class: None,
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
    assert_eq!(fake.exec(&h, &a, DL), Ok(ExecOutcome::Exited { code: 0 }));
    assert_eq!(fake.exec(&h, &b, DL), Ok(ExecOutcome::Exited { code: 0 }));
    assert_eq!(fake.execs(), [(h, a, DL), (BoxHandle::new("box-1"), b, DL)]);
    assert_eq!(fake.calls(), ["acquire", "exec box-1", "exec box-1"]);
}

#[test]
fn scripted_exec_outcomes_are_fifo_then_default_to_success() {
    let (fake, h) = fake_with_box();
    let fake = fake.exec_results([Ok(ExecOutcome::Exited { code: 3 })]);
    assert_eq!(
        fake.exec(&h, &spec(&["x"]), DL),
        Ok(ExecOutcome::Exited { code: 3 })
    );
    assert_eq!(
        fake.exec(&h, &spec(&["x"]), DL),
        Ok(ExecOutcome::Exited { code: 0 })
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

#[test]
fn exec_records_sync_output_host_cwd_and_class() {
    let (fake, h) = fake_with_box();
    let mut s = spec(&["x"]);
    s.sync = SyncMode::None;
    s.output = Some(Output::Log(PathBuf::from("/log/x")));
    s.host_cwd = Some(PathBuf::from("/host"));
    s.class = Some("big".to_owned());
    fake.exec(&h, &s, DL).unwrap();
    assert_eq!(fake.execs(), [(h, s, DL)]);
}

#[test]
fn scripted_interrupted_outcome_is_returned() {
    let (fake, h) = fake_with_box();
    let fake = fake.exec_results([Ok(ExecOutcome::Interrupted)]);
    assert_eq!(
        fake.exec(&h, &spec(&["x"]), DL),
        Ok(ExecOutcome::Interrupted)
    );
}

fn copy(fake: &FakeDriver, h: &BoxHandle) -> Result<(), DriverError> {
    let from = BoxPath::under("/work", "out").unwrap();
    fake.copy_out(h, &from, &PathBuf::from("/dst"), &PathBuf::from("/host"))
}

#[test]
fn copy_out_records_its_arguments() {
    let (fake, h) = fake_with_box();
    assert_eq!(copy(&fake, &h), Ok(()));
    let from = BoxPath::under("/work", "out").unwrap();
    let want = (h, from, PathBuf::from("/dst"), PathBuf::from("/host"));
    assert_eq!(fake.copies(), [want]);
    assert_eq!(fake.calls(), ["acquire", "copy_out box-1"]);
}

#[test]
fn copy_out_on_unknown_handle_is_not_found() {
    let fake = FakeDriver::new();
    assert_eq!(
        copy(&fake, &BoxHandle::new("box-9")),
        Err(DriverError::NotFound)
    );
}

#[test]
fn scripted_copy_out_failure_is_one_shot() {
    let (fake, h) = fake_with_box();
    let fake = fake.fail_copy_out(DriverError::Quota("q".into()));
    assert_eq!(copy(&fake, &h), Err(DriverError::Quota("q".into())));
    assert_eq!(copy(&fake, &h), Ok(()));
}

fn host() -> &'static Path {
    Path::new("/host")
}

fn peeked(rel: &str) -> BoxPath {
    BoxPath::under("/work", rel).unwrap()
}

#[test]
fn peek_serves_text_per_box_and_records_reads() {
    let fake = FakeDriver::new();
    let a = fake.acquire(&req()).unwrap().handle;
    let b = fake.acquire(&req()).unwrap().handle;
    let path = peeked("s.json");
    let fake = fake.peek_text("box-1", &path, "one");
    assert_eq!(
        fake.peek_file(&a, &path, host()),
        Ok(Some("one".to_owned()))
    );
    assert_eq!(fake.peek_file(&b, &path, host()), Ok(None));
    assert_eq!(fake.peek_file(&a, &peeked("other"), host()), Ok(None));
    assert_eq!(fake.peeks().len(), 3);
    assert_eq!(
        fake.peeks().first(),
        Some(&(a, path, PathBuf::from("/host")))
    );
}

#[test]
fn peek_on_unknown_handle_is_not_found_and_scripted_failure_is_one_shot() {
    let fake = FakeDriver::new();
    assert_eq!(
        fake.peek_file(&BoxHandle::new("box-9"), &peeked("x"), host()),
        Err(DriverError::NotFound)
    );
    let (fake, h) = fake_with_box();
    let fake = fake.fail_peek(DriverError::Timeout(Duration::from_secs(1)));
    assert_eq!(
        fake.peek_file(&h, &peeked("x"), host()),
        Err(DriverError::Timeout(Duration::from_secs(1)))
    );
    assert_eq!(fake.peek_file(&h, &peeked("x"), host()), Ok(None));
}

#[test]
fn peek_returns_while_an_exec_on_the_same_box_is_blocked() {
    let (fake, h) = fake_with_box();
    let path = peeked("s.json");
    let fake = Arc::new(fake.peek_text("box-1", &path, "live"));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = std::sync::Mutex::new(release_rx);
    fake.on_exec(move || {
        entered_tx.send(()).unwrap();
        let _ = release_rx.lock().unwrap().recv();
    });
    let runner = {
        let (fake, h) = (fake.clone(), h.clone());
        std::thread::spawn(move || fake.exec(&h, &spec(&["sleep"]), DL))
    };
    entered_rx.recv().unwrap();
    assert_eq!(
        fake.peek_file(&h, &path, host()),
        Ok(Some("live".to_owned()))
    );
    release_tx.send(()).unwrap();
    assert_eq!(runner.join().unwrap(), Ok(ExecOutcome::Exited { code: 0 }));
}
