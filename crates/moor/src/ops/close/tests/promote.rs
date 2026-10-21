use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use super::is_working;

use moor_core::boxscript::setup::setup_script;
use moor_core::config::{BoxConfig, DriverConfig, UserConfig};
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::seam::proc::Tool;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, CloseReq, ProviderId, ShowReq, StartReq};

use crate::cx::Cx;
use crate::ops::close::run as close;
use crate::ops::common::boxlock::{BoxLock, WAIT};
use crate::ops::common::promote::PROMOTE_WAIT;
use crate::ops::show;
use crate::ops::start;
use crate::testing::cx::fake_config;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

const PROVIDER: &str = "local-container";
const HOLDER_BOX: &str = "box-1";
const HOLDER_WORKDIR: &str = "/work/box-1";
/// The host commit id the close probe compares against.
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

struct World {
    cx: Cx,
    driver: Arc<FakeDriver>,
    holder: BoxId,
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
                lookup_timeout_secs: None,
                strip_env: Vec::new(),
            },
        )]),
        ..UserConfig::default()
    }
}

/// A live machine held by Box `holder`, bound to `config` and `driver`.
fn world(config: UserConfig, driver: FakeDriver) -> World {
    world_at(config, driver, Path::new("/repo"))
}

/// Like [`world`], over the repo checked out at `repo`.
fn world_at(config: UserConfig, driver: FakeDriver, repo: &Path) -> World {
    world_failing(config, driver, None, repo)
}

/// Like [`world_at`]; the next acquire after the holder's fails with `acquire_err`.
fn world_failing(
    config: UserConfig,
    driver: FakeDriver,
    acquire_err: Option<DriverError>,
    repo: &Path,
) -> World {
    let dir = tempfile::tempdir().unwrap();
    // The holder is the driver's first Box, so close probes `box-1` at its recorded workdir.
    let probe = BoxPath::under(HOLDER_WORKDIR, ".moor/boxwork").unwrap();
    let driver = driver.peek_text(HOLDER_BOX, &probe, "ahead=0 dirty=0");
    let proc = ScriptedProc::new();
    let repo_out = |s: &str| Reply::Ok(format!("{}{s}\n", repo.display()));
    proc.on(Tool::Git, &["rev-parse", "--show-toplevel"], repo_out(""));
    proc.on(
        Tool::Git,
        &["rev-parse", "HEAD"],
        Reply::Ok(format!("{HEAD}\n")),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        repo_out("/.git"),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "--git-path", "info/exclude"],
        Reply::Ok(format!("{}/exclude\n", dir.path().display())),
    );
    proc.on(Tool::Git, &["rev-list"], Reply::Ok("0\n".into()));
    proc.on(Tool::Git, &["status"], Reply::Ok(String::new()));
    proc.on(Tool::Git, &["ls-tree"], Reply::Ok(String::new()));
    let handle = acquire_holder(&driver);
    let driver = Arc::new(match acquire_err {
        Some(e) => driver.fail_acquire(e),
        None => driver,
    });
    let cx = Cx::fake_with(
        dir.path(),
        repo,
        driver.clone(),
        Arc::new(FakeSurface::new(dir.path())),
        Arc::new(proc),
        config,
    );
    let holder = insert_holder(&cx, handle, dir.path());
    World {
        cx,
        driver,
        holder,
        _dir: dir,
    }
}

/// Acquires the holder's Box, so the driver knows it and can run the close probe in it.
fn acquire_holder(driver: &FakeDriver) -> BoxHandle {
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new(PROVIDER),
            box_id: BoxId::new(1),
            name: BoxName::parse_allow_bare("holder").unwrap(),
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    assert_eq!(got.handle.as_str(), HOLDER_BOX);
    got.handle
}

fn insert_holder(cx: &Cx, handle: BoxHandle, worktree: &Path) -> BoxId {
    let store = cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("holder").unwrap(),
            repo: "/repo".into(),
        }))
        .unwrap()
        .box_id;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new(PROVIDER)),
        worktree: Field::Set(worktree.display().to_string()),
        box_workdir: Field::Set(HOLDER_WORKDIR.into()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired { handle });
    store.apply(d).unwrap();
    id
}

fn start_req(name: &str) -> StartReq {
    StartReq {
        name: BoxName::parse_allow_bare(name).unwrap(),
        goal: None,
        issue: None,
        worktree: None,
        base: None,
        model: None,
        no_agent: false,
        hold: false,
        resume: false,
        trust_moor_yaml: false,
        repo: None,
        parent: None,
        identity: None,
    }
}

/// Queues `name` through `start` at the limit.
fn queue(w: &World, name: &str) -> BoxId {
    let v = start::run(&w.cx, &start_req(name)).unwrap();
    assert!(v.queued, "{name} should queue");
    BoxId::new(v.boxed.id)
}

/// A waiting Box made without consulting any limit.
fn waiting_directly(w: &World, name: &str) -> BoxId {
    let store = w.cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare(name).unwrap(),
            repo: "/repo".into(),
        }))
        .unwrap()
        .box_id;
    let d = Decision::new(id).lifecycle(BoxLifecycleInput::AtCapacity { max_boxes: 1 });
    store.apply(d).unwrap();
    id
}

fn close_holder(w: &World) -> moor_core::view::CloseView {
    let req = CloseReq {
        target: BoxName::parse_allow_bare("holder").unwrap().into(),
        force: false,
        trust_moor_yaml: false,
    };
    close(&w.cx, &req).unwrap()
}

fn row_of(w: &World, id: BoxId) -> crate::store::BoxRow {
    w.cx.store().unwrap().box_row(id).unwrap().unwrap()
}

fn lifecycle_of(w: &World, id: BoxId) -> BoxLifecycle {
    row_of(w, id).lifecycle
}

fn kinds(w: &World, id: BoxId) -> Vec<String> {
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

fn acquires(w: &World) -> usize {
    // The holder's own acquire is not counted.
    w.driver.calls().iter().filter(|c| *c == "acquire").count() - 1
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
    assert_eq!(lifecycle_of(&w, second), BoxLifecycle::WaitingForBox);
}

#[test]
fn a_younger_waiter_stays_queued_when_one_slot_frees() {
    let w = world(limit(1), FakeDriver::new());
    let first = queue(&w, "first");
    let second = queue(&w, "second");
    close_holder(&w);
    assert_ne!(lifecycle_of(&w, first), BoxLifecycle::WaitingForBox);
    assert_eq!(lifecycle_of(&w, second), BoxLifecycle::WaitingForBox);
    assert_eq!(acquires(&w), 1);
}

#[test]
fn promoted_box_goes_through_setting_up_and_gets_a_ready_box() {
    let w = world(limit(1), FakeDriver::new());
    let id = queue(&w, "first");
    close_holder(&w);
    let seen = kinds(&w, id);
    let at = |k: &str| seen.iter().position(|s| s == k);
    assert!(at("waiting_for_box") < at("setting_up"), "{seen:?}");
    assert!(at("setting_up") < at("working"), "{seen:?}");
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert!(is_working(&row));
    let b = row.boxed.expect("promoted Box has a machine row");
    assert_eq!(b.state, BoxState::Ready);
    assert!(b.handle.is_some());
    // The holder's acquire, two close probes and report collection come first; each is one exec and one peek,
    // and the close collects artifacts with one more exec.
    assert_eq!(
        w.driver.calls(),
        [
            "acquire".to_owned(),
            format!("exec {HOLDER_BOX}"),
            format!("peek_file {HOLDER_BOX}"),
            format!("exec {HOLDER_BOX}"),
            format!("peek_file {HOLDER_BOX}"),
            format!("exec {HOLDER_BOX}"),
            format!("peek_file {HOLDER_BOX}"),
            format!("exec {HOLDER_BOX}"),
            format!("release {HOLDER_BOX}"),
            "acquire".into(),
            // The promoted Box has no setup, so its one exec is the checkout sync.
            "exec box-2".into(),
        ]
    );
}

#[test]
fn a_waiter_on_a_provider_without_reverse_tunnel_is_not_provisioned() {
    let w = world(limit(1), FakeDriver::new().without_reverse_tunnel_cap());
    let id = waiting_directly(&w, "first");
    let v = close_holder(&w);
    assert!(v.promoted.is_empty());
    assert_eq!(acquires(&w), 0);
    assert!(!is_working(&row_of(&w, id)));
}

#[test]
fn driver_limit_frees_a_slot_for_the_waiter_too() {
    let w = world(driver_limit(1), FakeDriver::new());
    queue(&w, "first");
    let v = close_holder(&w);
    assert_eq!(v.promoted, ["first"]);
}

#[test]
fn failed_acquire_of_the_promoted_box_does_not_fail_the_close() {
    let failure = DriverError::Failed {
        summary: "no capacity".into(),
    };
    let w = world_failing(
        limit(1),
        FakeDriver::new(),
        Some(failure),
        Path::new("/repo"),
    );
    let id = queue(&w, "first");
    let v = close_holder(&w);
    assert!(v.box_deleted);
    assert!(!v.promoted.contains(&"first".to_owned()));
    assert!(warning_codes(&w).contains(&"promote_failed".to_owned()));
    assert!(!is_working(&row_of(&w, id)));
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
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    };
    show::run(&w.cx, &req).unwrap();
    assert_eq!(lifecycle_of(&w, id), BoxLifecycle::WaitingForBox);
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
    assert_eq!(lifecycle_of(&w, id), BoxLifecycle::WaitingForBox);
    assert_eq!(acquires(&w), 0);
}

#[test]
fn nothing_is_promoted_without_a_configured_limit() {
    let w = world(UserConfig::default(), FakeDriver::new());
    let id = waiting_directly(&w, "first");
    let v = close_holder(&w);
    assert!(v.promoted.is_empty());
    assert_eq!(lifecycle_of(&w, id), BoxLifecycle::WaitingForBox);
    assert_eq!(acquires(&w), 0);
}

#[test]
fn close_with_no_waiting_boxes_promotes_nothing() {
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
    assert_eq!(lifecycle_of(&w, id), BoxLifecycle::WaitingForBox);
    assert_eq!(acquires(&w), 0);
}

#[test]
fn a_store_read_error_during_promotion_is_a_warning_not_a_failed_close() {
    let w = world(limit(1), FakeDriver::new());
    queue(&w, "first");
    let db = rusqlite::Connection::open(w.cx.state_dir().unwrap().join("moor.db")).unwrap();
    db.execute(
        "UPDATE boxes SET name = '' WHERE state = 'waiting_for_box'",
        [],
    )
    .unwrap();
    let v = close_holder(&w);
    assert!(v.box_deleted);
    assert!(v.promoted.is_empty());
    assert!(warning_codes(&w).contains(&"promote_failed".to_owned()));
}

#[test]
fn promoted_box_runs_the_repo_setup_commands_in_its_new_box() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(
        repo.path().join(".moor.yaml"),
        "setup: [\"echo one\", \"echo two\"]\n",
    )
    .unwrap();
    // Setup runs through the Proxy, which stages its credentials under the Box user's HOME.
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let driver = FakeDriver::new().peek_text("box-2", &environ, "HOME=/home/box\0");
    let config = UserConfig {
        credentials: fake_config().credentials,
        ..limit(1)
    };
    let w = world_at(config, driver, repo.path());
    let id = queue(&w, "first");
    close_holder(&w);
    assert!(is_working(&row_of(&w, id)));
    let handle = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    let handle = handle.boxed.unwrap().handle.expect("promoted Box handle");
    let argvs: Vec<_> = w
        .driver
        .execs()
        .into_iter()
        .filter(|(h, spec, _)| *h == handle && spec.argv != ["git", "rev-parse", "--git-dir"])
        .map(|(_, spec, _)| spec.argv)
        .collect();
    let steps = ["echo one", "echo two"].map(str::to_owned);
    let script = setup_script(&steps);
    let ran = argvs
        .iter()
        .filter(|a| a.last().is_some_and(|l| l.contains(&script)))
        .count();
    assert_eq!(ran, 1, "{argvs:?}");
}
