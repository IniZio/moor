//! Self-tests for the Driver fake.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moor_core::name::BoxName;
use moor_core::seam::driver::{
    AcquireReq, BoxStep, Driver, DriverError, ExecOutcome, ExecSpec, Released, SleepOutcome,
    StepDone,
};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::{BoxHandle, BoxId, BoxPath, ProviderId};

use super::FakeDriver;

fn req() -> AcquireReq {
    AcquireReq {
        provider: ProviderId::new("fake"),
        box_id: BoxId::new(1),
        name: BoxName::parse_allow_bare("t1").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
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
        output: None,
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
fn exec_records_output() {
    let (fake, h) = fake_with_box();
    let mut s = spec(&["x"]);
    s.output = Some(Output::Log(PathBuf::from("/log/x")));
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
    fake.copy_out(h, &from, &PathBuf::from("/dst"))
}

#[test]
fn copy_out_records_its_arguments() {
    let (fake, h) = fake_with_box();
    assert_eq!(copy(&fake, &h), Ok(()));
    let from = BoxPath::under("/work", "out").unwrap();
    let want = (h, from, PathBuf::from("/dst"));
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
    assert_eq!(fake.peek_file(&a, &path), Ok(Some("one".to_owned())));
    assert_eq!(fake.peek_file(&b, &path), Ok(None));
    assert_eq!(fake.peek_file(&a, &peeked("other")), Ok(None));
    assert_eq!(fake.peeks().len(), 3);
    assert_eq!(fake.peeks().first(), Some(&(a, path)));
}

#[test]
fn peek_on_unknown_handle_is_not_found_and_scripted_failure_is_one_shot() {
    let fake = FakeDriver::new();
    assert_eq!(
        fake.peek_file(&BoxHandle::new("box-9"), &peeked("x")),
        Err(DriverError::NotFound)
    );
    let (fake, h) = fake_with_box();
    let fake = fake.fail_peek(DriverError::Timeout(Duration::from_secs(1)));
    assert_eq!(
        fake.peek_file(&h, &peeked("x")),
        Err(DriverError::Timeout(Duration::from_secs(1)))
    );
    assert_eq!(fake.peek_file(&h, &peeked("x")), Ok(None));
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
    assert_eq!(fake.peek_file(&h, &path), Ok(Some("live".to_owned())));
    release_tx.send(()).unwrap();
    assert_eq!(runner.join().unwrap(), Ok(ExecOutcome::Exited { code: 0 }));
}

#[test]
fn sleep_and_wake_are_recorded_and_can_fail_once() {
    let fake = FakeDriver::new()
        .fail_sleep(DriverError::Unavailable("x".into()))
        .fail_wake(DriverError::Timeout(Duration::from_secs(1)));
    let h = fake.acquire(&req()).unwrap().handle;
    assert_eq!(fake.sleep(&h), Err(DriverError::Unavailable("x".into())));
    assert_eq!(fake.sleep(&h), Ok(SleepOutcome::Slept));
    assert_eq!(
        fake.wake(&h),
        Err(DriverError::Timeout(Duration::from_secs(1)))
    );
    assert_eq!(fake.wake(&h), Ok(()));
    assert_eq!(
        fake.calls(),
        [
            "acquire",
            "sleep box-1",
            "sleep box-1",
            "wake box-1",
            "wake box-1"
        ]
    );
}

#[test]
fn sleep_outcome_is_scripted_and_gone_box_is_not_found() {
    let fake = FakeDriver::new().sleep_outcome(SleepOutcome::Native);
    let h = fake.acquire(&req()).unwrap().handle;
    assert_eq!(fake.sleep(&h), Ok(SleepOutcome::Native));
    fake.forget(&h);
    assert_eq!(fake.sleep(&h), Err(DriverError::NotFound));
    assert_eq!(fake.wake(&h), Err(DriverError::NotFound));
}

#[test]
fn put_is_served_back_by_peek_per_box_and_recorded() {
    let (fake, a) = fake_with_box();
    let b = fake.acquire(&req()).unwrap().handle;
    let path = BoxPath::restore("/home/u/.moor-cred-x/brief.md").unwrap();
    assert_eq!(fake.put_file(&a, &path, b"one", 0o600), Ok(()));
    assert_eq!(fake.put_file(&a, &path, b"two", 0o600), Ok(()));
    assert_eq!(fake.peek_file(&a, &path), Ok(Some("two".to_owned())));
    assert_eq!(fake.peek_file(&b, &path), Ok(None));
    assert_eq!(
        fake.puts().first(),
        Some(&(a, path, b"one".to_vec(), 0o600))
    );
}

#[test]
fn put_and_connect_on_unknown_box_are_not_found_and_failures_are_one_shot() {
    let fake = FakeDriver::new();
    let gone = BoxHandle::new("box-9");
    let path = BoxPath::restore("/h/f").unwrap();
    assert_eq!(
        fake.put_file(&gone, &path, b"x", 0o600),
        Err(DriverError::NotFound)
    );
    assert_eq!(fake.connect_argv(&gone), Err(DriverError::NotFound));
    let (fake, h) = fake_with_box();
    let fake = fake
        .fail_put(DriverError::Quota("q".into()))
        .fail_connect(DriverError::Quota("q".into()));
    assert!(fake.put_file(&h, &path, b"x", 0o600).is_err());
    assert!(fake.put_file(&h, &path, b"x", 0o600).is_ok());
    assert!(fake.connect_argv(&h).is_err());
    assert_eq!(
        fake.connect_argv(&h),
        Ok(vec!["ssh".to_owned(), "-t".to_owned(), "box-1".to_owned()])
    );
}

#[test]
fn workdir_uses_a_lease_id_that_differs_from_the_handle() {
    let fake = FakeDriver::new();
    let r = req();
    let got = fake.acquire(&r).unwrap();
    assert_eq!(got.workdir.as_str(), "/work/cbx_1/repo");
    let again = fake.workdir(&got.handle).unwrap();
    assert_eq!(again, got.workdir);
    assert_ne!(again.as_str(), "/work/box-1");
}

#[test]
fn on_wake_runs_after_a_successful_wake_only() {
    let fake = Arc::new(FakeDriver::new().fail_wake(DriverError::Timeout(Duration::from_secs(1))));
    let h = fake.acquire(&req()).unwrap().handle;
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = hits.clone();
    fake.on_wake(move || {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    assert!(fake.wake(&h).is_err());
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(fake.wake(&h), Ok(()));
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn batch_dispatches_to_own_logs_and_counts_one_top_level_call() {
    let fake = FakeDriver::new().run_stdout("out");
    let h = fake.acquire(&req()).unwrap().handle;
    let path = BoxPath::under("/work", "a").unwrap();
    let steps = [
        BoxStep::Peek(path.clone()),
        BoxStep::Put {
            path,
            bytes: b"x".to_vec(),
            mode: 0o600,
        },
        BoxStep::Run("true".to_owned()),
    ];
    let connect = vec!["ssh".to_owned()];
    let done = fake.batch(&h, &connect, &steps, DL);
    assert_eq!(done.len(), 3);
    assert_eq!(
        done.last(),
        Some(&Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            stdout: "out".to_owned()
        }))
    );
    assert_eq!(fake.box_calls(), ["acquire", "batch"]);
    assert_eq!(fake.batch_connects(), [connect]);
    assert_eq!(fake.peeks().len(), 1);
    assert_eq!(fake.puts().len(), 1);
    assert_eq!(fake.execs().len(), 1);
}

#[test]
fn batch_stops_after_a_failed_run() {
    let fake = FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 1 })]);
    let h = fake.acquire(&req()).unwrap().handle;
    let steps = [BoxStep::Run("a".to_owned()), BoxStep::Run("b".to_owned())];
    let done = fake.batch(&h, &[], &steps, DL);
    assert_eq!(done.len(), 1);
    assert_eq!(fake.execs().len(), 1);
}

#[test]
fn artifact_index_text_goes_only_to_the_artifacts_exec() {
    let (fake, h) = fake_with_box();
    let fake = fake.exec_stdout(["plain"]).artifact_index(["listing"]);
    let dir = tempfile::tempdir().unwrap();
    let run = |script: &str, name: &str| {
        let log = dir.path().join(name);
        let mut s = spec(&["sh", "-c", script]);
        s.output = Some(Output::Log(log.clone()));
        fake.exec(&h, &s, DL).unwrap();
        std::fs::read_to_string(log).unwrap_or_default()
    };
    assert_eq!(run("ls .moor/artifacts", "a1"), "listing");
    assert_eq!(run("echo hi", "o1"), "plain");
    assert_eq!(run("ls .moor/artifacts", "a2"), "");
    fake.push_artifact_index("later");
    assert_eq!(run("echo hi", "o2"), "");
    assert_eq!(run("ls .moor/artifacts", "a3"), "later");
    assert_eq!(run("echo hi", "o3"), "");
}
