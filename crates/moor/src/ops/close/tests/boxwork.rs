//! Close checks the Box for work that exists nowhere else.

use moor_core::boxscript::boxwork::BOXWORK_FILE;
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::seam::driver::DriverError;
use moor_core::{CloseReq, ErrorKind, ProviderId, TaskName};

use std::sync::Arc;

use moor_core::BoxPath;

use super::{admit, box_state, fx_box, proc, req, row, run, Fx};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

fn surface() -> FakeSurface {
    FakeSurface::new("/s")
}

/// A Ready Box whose probe answers `text`; `None` leaves the answer unreadable.
fn fx_probe(text: Option<&str>) -> Fx {
    fx_box(FakeDriver::new(), surface(), 0, text)
}

/// A Ready Box in a worktree named `repo`, so a woken Box (which the fake driver
/// places at `/work/cbx_1/repo`) reads `text` from its probe result.
fn fx_woken(text: &str) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().join("repo");
    std::fs::create_dir(&worktree).unwrap();
    let path = BoxPath::under("/work/cbx_1/repo", ".moor/boxwork").unwrap();
    let driver = Arc::new(FakeDriver::new().peek_text("box-1", &path, text));
    let surface = Arc::new(surface());
    let proc = proc(0);
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        driver.clone(),
        surface.clone(),
        proc.clone(),
    );
    let id = admit(&cx, &driver, &worktree);
    Fx {
        dir,
        cx,
        driver,
        surface,
        proc,
        id,
    }
}

/// Puts the Task and its Box to sleep.
fn put_to_sleep(f: &Fx) {
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).task_input(TaskInput::AgentQuiet))
        .unwrap();
    store
        .apply(
            Decision::new(f.id)
                .task_input(TaskInput::SleepCmd)
                .box_input(BoxInput::Sleep),
        )
        .unwrap();
    assert_eq!(box_state(f), BoxState::Asleep);
}

fn count(f: &Fx, prefix: &str) -> usize {
    f.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with(prefix))
        .count()
}

/// Execs that ran the unsaved-work probe, as opposed to other closing steps.
fn probes(f: &Fx) -> usize {
    f.driver
        .execs()
        .iter()
        .filter(|(_, spec, _)| spec.argv.iter().any(|a| a.contains(BOXWORK_FILE)))
        .count()
}

/// The error message with its hints, as a user reads them.
fn text(e: &CommandError) -> String {
    format!("{}\n{}", e.error, e.hints.join("\n"))
}

/// Closing without `--force` is refused and changes nothing.
fn refused(f: &Fx) -> CommandError {
    let (task, boxed) = (row(f).state, box_state(f));
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert_eq!(row(f).state, task);
    assert_eq!(box_state(f), boxed);
    assert_eq!(count(f, "release"), 0);
    e
}

#[test]
fn close_refuses_a_box_with_an_unpushed_commit_when_the_host_worktree_is_clean() {
    let f = fx_probe(Some("ahead=1 dirty=0"));
    let t = text(&refused(&f));
    assert!(t.contains("the Box holds work"), "{t}");
    assert!(t.contains("not on the host"), "{t}");
    assert!(t.contains("1 unpushed commit"), "{t}");
    for cmd in [
        "moor run t --no-sync -- git bundle create .moor/rescue.bundle HEAD --not --remotes",
        "moor cp t .moor/rescue.bundle .",
        "git fetch ./rescue.bundle HEAD:",
        "moor close t --force",
    ] {
        assert!(t.contains(cmd), "missing {cmd:?} in {t}");
    }
    assert_eq!(count(&f, "exec"), 1);
}

#[test]
fn close_refuses_a_box_with_only_uncommitted_changes() {
    let f = fx_probe(Some("ahead=0 dirty=3"));
    let t = text(&refused(&f));
    assert!(t.contains("the Box holds work"), "{t}");
    assert!(t.contains("3 uncommitted file"), "{t}");
    assert!(t.contains("git push moor HEAD:refs/heads/"), "{t}");
    assert!(!t.contains("bundle"), "{t}");
}

#[test]
fn close_proceeds_when_the_box_probe_is_clean() {
    let f = fx_probe(Some("ahead=0 dirty=0"));
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert_eq!(box_state(&f), BoxState::Deleted);
    assert_eq!(probes(&f), 2, "the probe ran before and after the stop");
}

#[test]
fn close_refuses_when_the_box_cannot_be_checked() {
    let f = fx_probe(None);
    let t = text(&refused(&f));
    assert!(
        t.contains("could not check the Box for unsaved work"),
        "{t}"
    );
}

#[test]
fn close_wakes_a_sleeping_box_and_refuses_when_it_holds_unsaved_work() {
    let f = fx_woken("ahead=2 dirty=0");
    put_to_sleep(&f);
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(text(&e).contains("the Box holds work"), "{}", text(&e));
    assert_eq!(count(&f, "wake"), 1);
    assert_eq!(count(&f, "exec"), 1);
    assert_ne!(row(&f).state, TaskState::Closed);
    assert_ne!(box_state(&f), BoxState::Deleted);
    assert_eq!(count(&f, "release"), 0);
}

#[test]
fn close_wakes_a_sleeping_box_and_proceeds_when_it_is_clean() {
    let f = fx_woken("ahead=0 dirty=0");
    put_to_sleep(&f);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert_eq!(count(&f, "wake"), 1);
    assert_eq!(probes(&f), 2);
}

#[test]
fn close_refuses_and_leaves_the_box_asleep_when_it_cannot_be_woken() {
    let driver = FakeDriver::new().fail_wake(DriverError::Failed {
        summary: "boom".into(),
    });
    let f = fx_box(driver, surface(), 0, Some("ahead=0 dirty=0"));
    put_to_sleep(&f);
    let t = text(&refused(&f));
    assert!(t.contains("could not wake the Box"), "{t}");
    assert_eq!(box_state(&f), BoxState::Asleep);
    assert_eq!(count(&f, "exec"), 0);
}

#[test]
fn close_proceeds_when_the_driver_no_longer_has_the_box_on_probe() {
    let f = fx_probe(Some("ahead=1 dirty=1"));
    let h = row(&f).boxed.unwrap().handle.unwrap();
    f.driver.forget(&h);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
}

#[test]
fn close_proceeds_when_the_driver_no_longer_has_the_box_on_wake() {
    let driver = FakeDriver::new().fail_wake(DriverError::NotFound);
    let f = fx_box(driver, surface(), 0, Some("ahead=0 dirty=0"));
    put_to_sleep(&f);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
}

#[test]
fn force_closes_a_box_with_unsaved_work_without_probing() {
    let f = fx_probe(Some("ahead=4 dirty=1"));
    let v = run(&f.cx, &req(true)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert_eq!(probes(&f), 0);
    assert_eq!(count(&f, "wake"), 0);
}

#[test]
fn force_closes_a_sleeping_box_without_waking_or_probing_it() {
    let f = fx_probe(Some("ahead=4 dirty=1"));
    put_to_sleep(&f);
    let v = run(&f.cx, &req(true)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert_eq!(count(&f, "exec"), 0);
    assert_eq!(count(&f, "wake"), 0);
}

#[test]
fn close_skips_the_probe_for_a_box_the_driver_no_longer_has() {
    let f = fx_probe(Some("ahead=1 dirty=1"));
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).box_input(BoxInput::DriverMissing))
        .unwrap();
    assert_eq!(box_state(&f), BoxState::Gone);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert_eq!(count(&f, "exec"), 0);
}

#[test]
fn close_skips_the_probe_for_a_box_that_never_got_a_handle() {
    let f = fx_probe(Some("ahead=1 dirty=1"));
    let store = f.cx.store().unwrap();
    let name = TaskName::parse("u").unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
        ..Fields::default()
    };
    store
        .apply(
            Decision::new(id)
                .task_input(TaskInput::Admitted)
                .box_input(BoxInput::Requested)
                .set(fields),
        )
        .unwrap();
    let cause = "no capacity".to_owned();
    store
        .apply(
            Decision::new(id)
                .task_input(TaskInput::AcquireFailed {
                    cause: cause.clone(),
                })
                .box_input(BoxInput::AcquireFailed {
                    cause,
                    attempted: None,
                }),
        )
        .unwrap();
    let v = run(
        &f.cx,
        &CloseReq {
            task: name,
            force: false,
            trust_moor_yaml: false,
        },
    )
    .unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert_eq!(count(&f, "exec"), 0);
}

#[test]
fn close_refuses_when_the_agent_writes_between_the_probe_and_the_stop() {
    let f = fx_probe(Some("ahead=0 dirty=0"));
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "s-1", Some("p-gone"))
        .unwrap();
    let driver = f.driver.clone();
    let runs = std::sync::atomic::AtomicU32::new(0);
    f.driver.on_exec(move || {
        if runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            let path = BoxPath::under("/work/box-1", ".moor/boxwork").unwrap();
            driver.set_peek_text("box-1", &path, "ahead=1 dirty=2");
        }
    });
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    let t = text(&e);
    assert!(t.contains("the Box holds work"), "{t}");
    assert!(t.contains("the agent was stopped"), "{t}");
    assert!(t.contains("moor start t --resume"), "{t}");
    assert_eq!(count(&f, "exec"), 2);
    assert_eq!(count(&f, "release"), 0);
    assert_ne!(row(&f).state, TaskState::Closed);
    assert_ne!(box_state(&f), BoxState::Deleted);
    assert!(f.cx.store().unwrap().live_session(f.id).unwrap().is_none());
}

#[test]
fn force_does_not_probe_before_or_after_stopping_the_agent() {
    let f = fx_probe(Some("ahead=4 dirty=1"));
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "s-1", Some("p-gone"))
        .unwrap();
    let v = run(&f.cx, &req(true)).unwrap();
    assert!(v.agent_stopped);
    assert_eq!(probes(&f), 0);
}
