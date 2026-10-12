//! Tests for `cp`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::name::BoxPath;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, EgressPlan};
use moor_core::{BoxHandle, CpReq, ErrorKind, ProviderId, TaskId, TaskName};

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const CWD: &str = "/home/me/proj";

struct Fx {
    dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    id: TaskId,
}

/// Task `t`, inserted and admitted; `ready` also gives it a live Box.
fn fx_with(driver: FakeDriver, ready: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = Cx::fake(
        dir.path(),
        Path::new(CWD),
        driver.clone(),
        Arc::new(FakeSurface::new(PathBuf::from("/s"))),
        Arc::new(ScriptedProc::new()),
    );
    let store = cx.store().unwrap();
    let name = TaskName::parse("t").unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    let provider = ProviderId::new("p");
    let d = if ready {
        let fields = Fields {
            box_provider: Field::Set(provider.clone()),
            ..Fields::default()
        };
        Decision::new(id)
            .task_input(TaskInput::Admitted)
            .box_input(BoxInput::Requested)
            .set(fields)
    } else {
        Decision::new(id).task_input(TaskInput::AtCapacity { max_boxes: 1 })
    };
    store.apply(d).unwrap();
    if ready {
        let got = driver
            .acquire(&AcquireReq {
                provider,
                task: id,
                name,
                egress: EgressPlan::Open,
                class: None,
                nonce: "n".to_owned(),
                worktree: None,
            })
            .unwrap();
        let d = Decision::new(id)
            .task_input(TaskInput::BoxReady {
                setup: SetupOutcome::Ok,
            })
            .box_input(BoxInput::Acquired { handle: got.handle });
        store.apply(d).unwrap();
    }
    Fx {
        dir,
        cx,
        driver,
        id,
    }
}

fn fx() -> Fx {
    fx_with(FakeDriver::new(), true)
}

fn req(src: &str, dest: &str) -> CpReq {
    CpReq {
        task: TaskName::parse("t").unwrap(),
        src: src.into(),
        dest: dest.into(),
    }
}

/// Kinds of the events recorded for the task.
fn kinds(f: &Fx) -> Vec<String> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter().map(|e| e.kind).collect()
}

/// A destination under the fixture's own directory.
fn dest(f: &Fx) -> String {
    f.dir.path().join("out").display().to_string()
}

/// The first `copy_out` the Driver saw.
fn first_copy(f: &Fx) -> (BoxHandle, BoxPath, PathBuf, PathBuf) {
    f.driver.copies().into_iter().next().unwrap()
}

fn copied(f: &Fx) -> usize {
    kinds(f).iter().filter(|k| *k == "copied").count()
}

#[test]
fn relative_source_resolves_against_the_box_workdir() {
    let f = fx();
    run(&f.cx, &req("out/a.txt", "a.txt")).unwrap();
    assert_eq!(f.driver.copies().len(), 1);
    assert_eq!(first_copy(&f).1.as_str(), "/work/box-1/out/a.txt");
}

#[test]
fn absolute_source_is_used_as_given() {
    let f = fx();
    run(&f.cx, &req("/var/log/x", "x")).unwrap();
    assert_eq!(first_copy(&f).1.as_str(), "/var/log/x");
    assert!(!f.driver.calls().iter().any(|c| c.starts_with("workdir")));
}

#[test]
fn undeterminable_workdir_makes_no_copy() {
    let f = fx();
    f.driver.forget(&BoxHandle::new("box-1"));
    run(&f.cx, &req("a", "a")).unwrap_err();
    assert!(f.driver.copies().is_empty());
}

#[test]
fn source_climbing_out_of_the_workdir_is_usage() {
    let f = fx();
    let err = run(&f.cx, &req("../x", "x")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(f.driver.copies().is_empty());
}

#[test]
fn relative_destination_is_absolute_from_the_callers_cwd() {
    let f = fx();
    run(&f.cx, &req("a", "sub/a.txt")).unwrap();
    let (_, _, to, host_cwd) = first_copy(&f);
    assert_eq!(to, Path::new(CWD).join("sub/a.txt"));
    assert_eq!(host_cwd, Path::new(CWD));
}

#[test]
fn absolute_destination_is_kept() {
    let f = fx();
    let to = dest(&f);
    run(&f.cx, &req("a", &to)).unwrap();
    assert_eq!(first_copy(&f).2, Path::new(&to));
}

#[test]
fn unknown_task_is_refused_without_a_copy() {
    let f = fx();
    let mut r = req("a", "a");
    r.task = TaskName::parse("nope").unwrap();
    run(&f.cx, &r).unwrap_err();
    assert!(f.driver.copies().is_empty());
    assert_eq!(copied(&f), 0);
}

#[test]
fn blank_source_is_usage_without_a_copy() {
    let f = fx();
    let err = run(&f.cx, &req("  ", "a")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(f.driver.copies().is_empty());
    assert_eq!(copied(&f), 0);
}

#[test]
fn gone_box_is_refused_without_a_copy() {
    let f = fx();
    let d = Decision::new(f.id).box_input(BoxInput::DriverMissing);
    f.cx.store().unwrap().apply(d).unwrap();
    let err = run(&f.cx, &req("a", "a")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(f.driver.copies().is_empty());
    assert_eq!(copied(&f), 0);
}

#[test]
fn task_waiting_for_a_box_is_refused_without_a_copy() {
    let f = fx_with(FakeDriver::new(), false);
    let err = run(&f.cx, &req("a", "a")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(f.driver.copies().is_empty());
    assert_eq!(copied(&f), 0);
}

#[test]
fn success_records_one_copied_event_and_confirms() {
    let f = fx();
    let v = run(&f.cx, &req("a", &dest(&f))).unwrap();
    assert!(v.copied);
    assert_eq!(v.to, dest(&f));
    assert!(!v.confirmation.is_empty());
    assert_eq!(copied(&f), 1);
}

#[test]
fn failed_copy_still_records_one_copied_event() {
    let f = fx_with(
        FakeDriver::new().fail_copy_out(DriverError::Quota("q".into())),
        true,
    );
    let err = run(&f.cx, &req("a", &dest(&f))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert_eq!(copied(&f), 1);
}

#[test]
fn copy_refuses_while_another_op_holds_the_box_lock() {
    let f = fx();
    let _held = BoxLock::acquire(
        f.cx.clock(),
        f.cx.state_dir().unwrap(),
        f.id,
        "close",
        Duration::ZERO,
    )
    .unwrap();
    let err = run(&f.cx, &req("a", "a")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(f.driver.copies().is_empty());
}

#[test]
fn copy_never_syncs() {
    let f = fx();
    run(&f.cx, &req("a", &dest(&f))).unwrap();
    assert!(!f.driver.calls().iter().any(|c| c.starts_with("exec")));
}

#[test]
fn closed_task_with_a_release_failed_box_is_refused_without_a_copy() {
    let f = fx();
    let store = f.cx.store().unwrap();
    let d = Decision::new(f.id).box_input(BoxInput::ReleaseErr { cause: "x".into() });
    store.apply(d).unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::Close))
        .unwrap();
    let err = run(&f.cx, &req("a", "a")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(f.driver.copies().is_empty());
}

#[test]
fn box_that_is_not_ready_is_refused_without_a_copy() {
    let f = fx();
    let d = Decision::new(f.id).box_input(BoxInput::ReleaseErr { cause: "x".into() });
    f.cx.store().unwrap().apply(d).unwrap();
    assert_eq!(
        run(&f.cx, &req("a", "a")).unwrap_err().kind,
        ErrorKind::Refused
    );
    let g = fx();
    let d = Decision::new(g.id).box_input(BoxInput::Sleep);
    g.cx.store().unwrap().apply(d).unwrap();
    assert_eq!(
        run(&g.cx, &req("a", "a")).unwrap_err().kind,
        ErrorKind::Refused
    );
    assert!(f.driver.copies().is_empty() && g.driver.copies().is_empty());
}

#[test]
fn size_does_not_follow_directory_links() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f"), b"abc").unwrap();
    std::os::unix::fs::symlink(".", dir.path().join("loop")).unwrap();
    std::os::unix::fs::symlink("/", dir.path().join("root")).unwrap();
    let total = super::size(dir.path());
    assert!((3..1000).contains(&total), "{total}");
}
