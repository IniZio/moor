use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use moor_core::config::{BoxConfig, DriverConfig, UserConfig};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::Tool;
use moor_core::{BoxHandle, CloseReq, ProviderId, ShowReq, StartReq, TaskId, TaskName};

use crate::cx::Cx;
use crate::ops::close::run as close;
use crate::ops::common::boxlock::{BoxLock, WAIT};
use crate::ops::common::promote::PROMOTE_WAIT;
use crate::ops::show;
use crate::ops::start;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

const PROVIDER: &str = "local-container";
const HOLDER_BOX: &str = "holder-box";

struct World {
    cx: Cx,
    driver: Arc<FakeDriver>,
    holder: TaskId,
    _dir: tempfile::TempDir,
}

fn limit(n: u32) -> UserConfig {
    UserConfig {
        box_: BoxConfig {
            max_boxes: NonZeroU32::new(n),
            ..BoxConfig::default()
        },
        ..UserConfig::default()
    }
}

fn driver_limit(n: u32) -> UserConfig {
    UserConfig {
        drivers: BTreeMap::from([(
            PROVIDER.to_owned(),
            DriverConfig {
                max_boxes: NonZeroU32::new(n),
            },
        )]),
        ..UserConfig::default()
    }
}

/// A live Box held by task `holder`, bound to `config` and `driver`.
fn world(config: UserConfig, driver: FakeDriver) -> World {
    let repo = Path::new("/repo");
    let dir = tempfile::tempdir().unwrap();
    let proc = ScriptedProc::new();
    let repo_out = |s: &str| Reply::Ok(format!("{}{s}\n", repo.display()));
    proc.on(Tool::Git, &["rev-parse", "--show-toplevel"], repo_out(""));
    proc.on(
        Tool::Git,
        &["rev-parse", "HEAD"],
        Reply::Ok("abc123\n".into()),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        repo_out("/.git"),
    );
    proc.on(Tool::Git, &["rev-list"], Reply::Ok("0\n".into()));
    proc.on(Tool::Git, &["status"], Reply::Ok(String::new()));
    let driver = Arc::new(driver);
    let cx = Cx::fake_with(
        dir.path(),
        repo,
        driver.clone(),
        Arc::new(FakeSurface::new(dir.path())),
        Arc::new(proc),
        config,
    );
    let holder = insert_holder(&cx, dir.path());
    World {
        cx,
        driver,
        holder,
        _dir: dir,
    }
}

fn insert_holder(cx: &Cx, worktree: &Path) -> TaskId {
    let store = cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: TaskName::parse("holder").unwrap(),
            repo: "/repo".into(),
        }))
        .unwrap()
        .task;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new(PROVIDER)),
        worktree: Field::Set(worktree.display().to_string()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired {
            handle: BoxHandle::new(HOLDER_BOX),
        });
    store.apply(d).unwrap();
    id
}

fn start_req(name: &str) -> StartReq {
    StartReq {
        name: TaskName::parse(name).unwrap(),
        goal: None,
        issue: None,
        worktree: None,
        base: None,
        model: None,
        no_agent: false,
    }
}

/// Queues `name` through `start` at the limit.
fn queue(w: &World, name: &str) -> TaskId {
    let v = start::run(&w.cx, &start_req(name)).unwrap();
    assert!(v.queued, "{name} should queue");
    TaskId::new(v.task.id)
}

/// A waiting task made without consulting any limit.
fn waiting_directly(w: &World, name: &str) -> TaskId {
    let store = w.cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: TaskName::parse(name).unwrap(),
            repo: "/repo".into(),
        }))
        .unwrap()
        .task;
    let d = Decision::new(id).task_input(TaskInput::AtCapacity { max_boxes: 1 });
    store.apply(d).unwrap();
    id
}

fn close_holder(w: &World) -> moor_core::view::CloseView {
    let req = CloseReq {
        task: TaskName::parse("holder").unwrap(),
        force: false,
    };
    close(&w.cx, &req).unwrap()
}

fn state_of(w: &World, id: TaskId) -> TaskState {
    w.cx.store().unwrap().task(id).unwrap().unwrap().state
}

fn kinds(w: &World, id: TaskId) -> Vec<String> {
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

fn acquires(w: &World) -> usize {
    w.driver.calls().iter().filter(|c| *c == "acquire").count()
}

fn warning_codes(w: &World) -> Vec<String> {
    w.cx.take_warnings().into_iter().map(|x| x.code).collect()
}

#[test]
fn close_promotes_the_oldest_waiter_of_the_provider() {
    let w = world(limit(1), FakeDriver::new());
    queue(&w, "first");
    let second = queue(&w, "second");
    let v = close_holder(&w);
    assert!(v.box_deleted);
    assert_eq!(v.promoted, ["first"]);
    assert_eq!(state_of(&w, second), TaskState::WaitingForBox);
}

#[test]
fn a_younger_waiter_stays_queued_when_one_slot_frees() {
    let w = world(limit(1), FakeDriver::new());
    let first = queue(&w, "first");
    let second = queue(&w, "second");
    close_holder(&w);
    assert_ne!(state_of(&w, first), TaskState::WaitingForBox);
    assert_eq!(state_of(&w, second), TaskState::WaitingForBox);
    assert_eq!(acquires(&w), 1);
}

#[test]
fn promoted_task_goes_through_setting_up_and_gets_a_ready_box() {
    let w = world(limit(1), FakeDriver::new());
    let id = queue(&w, "first");
    close_holder(&w);
    let seen = kinds(&w, id);
    let at = |k: &str| seen.iter().position(|s| s == k);
    assert!(at("waiting_for_box") < at("setting_up"), "{seen:?}");
    assert!(at("setting_up") < at("working"), "{seen:?}");
    let row = w.cx.store().unwrap().task(id).unwrap().unwrap();
    assert_eq!(row.state, TaskState::Working);
    let b = row.boxed.expect("promoted task has a Box row");
    assert_eq!(b.state, BoxState::Ready);
    assert!(b.handle.is_some());
    assert_eq!(
        w.driver.calls(),
        [format!("release {HOLDER_BOX}"), "acquire".into()]
    );
}

#[test]
fn driver_limit_frees_a_slot_for_the_waiter_too() {
    let w = world(driver_limit(1), FakeDriver::new());
    queue(&w, "first");
    let v = close_holder(&w);
    assert_eq!(v.promoted, ["first"]);
}

#[test]
fn failed_acquire_of_the_promoted_task_does_not_fail_the_close() {
    let driver = FakeDriver::new().fail_acquire(DriverError::Failed {
        summary: "no capacity".into(),
    });
    let w = world(limit(1), driver);
    let id = queue(&w, "first");
    let v = close_holder(&w);
    assert!(v.box_deleted);
    assert!(!v.promoted.contains(&"first".to_owned()));
    assert!(warning_codes(&w).contains(&"promote_failed".to_owned()));
    assert_ne!(state_of(&w, id), TaskState::Working);
}

#[test]
fn show_does_not_promote_a_waiter_when_a_slot_is_free() {
    let w = world(limit(1), FakeDriver::new());
    let id = queue(&w, "first");
    w.cx.store()
        .unwrap()
        .apply(Decision::new(w.holder).box_input(BoxInput::DriverMissing))
        .unwrap();
    let req = ShowReq {
        task: None,
        events: None,
        agent: false,
        reports: false,
        pane: false,
    };
    show::run(&w.cx, &req).unwrap();
    assert_eq!(state_of(&w, id), TaskState::WaitingForBox);
    assert_eq!(acquires(&w), 0);
}

#[test]
fn a_waiter_is_not_promoted_when_the_freed_box_was_of_another_provider() {
    let w = world(limit(1), FakeDriver::new());
    let id = queue(&w, "first");
    let set = Fields {
        box_provider: Field::Set(ProviderId::new("other-provider")),
        ..Fields::default()
    };
    w.cx.store()
        .unwrap()
        .apply(Decision::new(w.holder).set(set))
        .unwrap();
    let v = close_holder(&w);
    assert!(v.box_deleted);
    assert!(v.promoted.is_empty());
    assert_eq!(state_of(&w, id), TaskState::WaitingForBox);
    assert_eq!(acquires(&w), 0);
}

#[test]
fn nothing_is_promoted_without_a_configured_limit() {
    let w = world(UserConfig::default(), FakeDriver::new());
    let id = waiting_directly(&w, "first");
    let v = close_holder(&w);
    assert!(v.promoted.is_empty());
    assert_eq!(state_of(&w, id), TaskState::WaitingForBox);
    assert_eq!(acquires(&w), 0);
}

#[test]
fn close_with_no_waiting_tasks_promotes_nothing() {
    let w = world(limit(1), FakeDriver::new());
    let v = close_holder(&w);
    assert!(v.box_deleted);
    assert!(v.promoted.is_empty());
    assert_eq!(acquires(&w), 0);
    assert!(warning_codes(&w).is_empty());
}

#[test]
fn a_waiter_whose_box_lock_is_held_is_skipped_with_a_warning() {
    let w = world(limit(1), FakeDriver::new());
    let id = queue(&w, "first");
    let _held =
        BoxLock::acquire(w.cx.clock(), w.cx.state_dir().unwrap(), id, "start", WAIT).unwrap();
    let from = w.cx.clock().instant();
    let v = close_holder(&w);
    let waited = w.cx.clock().instant() - from;
    assert!(waited >= PROMOTE_WAIT, "{waited:?}");
    assert!(waited < PROMOTE_WAIT * 2, "{waited:?}");
    assert!(v.box_deleted);
    assert!(v.promoted.is_empty());
    let warnings = w.cx.take_warnings();
    let skipped = warnings.iter().find(|x| x.code == "promote_skipped");
    let message = &skipped.expect("promote_skipped warning").message;
    assert!(message.starts_with("cannot promote first:"), "{message}");
    assert_eq!(state_of(&w, id), TaskState::WaitingForBox);
    assert_eq!(acquires(&w), 0);
}

#[test]
fn a_store_read_error_during_promotion_is_a_warning_not_a_failed_close() {
    let w = world(limit(1), FakeDriver::new());
    queue(&w, "first");
    let db = rusqlite::Connection::open(w.cx.state_dir().unwrap().join("moor.db")).unwrap();
    db.execute(
        "UPDATE tasks SET name = '' WHERE state = 'waiting_for_box'",
        [],
    )
    .unwrap();
    let v = close_holder(&w);
    assert!(v.box_deleted);
    assert!(v.promoted.is_empty());
    assert!(warning_codes(&w).contains(&"promote_failed".to_owned()));
}
