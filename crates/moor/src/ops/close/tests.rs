use std::sync::Arc;

use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::seam::proc::Tool;
use moor_core::{BoxId, BoxName, BoxPath, CloseReq, ErrorKind, ProviderId};

use super::{run, run_with_reason};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;

mod agent;
mod artifacts;
mod boxwork;
mod interrupt;
mod leases;
mod promote;
mod proxy;
mod reason;
mod reports;
mod target;
mod teardown;
mod transcript;
mod vanished;
mod wake;

const RAW: &str = "RAWDRIVER-7731";

pub(super) struct Fx {
    pub(super) dir: tempfile::TempDir,
    pub(super) cx: Cx,
    pub(super) driver: Arc<FakeDriver>,
    pub(super) proc: Arc<ScriptedProc>,
    pub(super) id: BoxId,
}

pub(super) fn req(force: bool) -> CloseReq {
    CloseReq {
        target: BoxName::parse_allow_bare("t").unwrap().into(),
        force,
    }
}

/// A host process table that answers no tool.
pub(super) fn proc() -> Arc<ScriptedProc> {
    Arc::new(ScriptedProc::new())
}

/// A Ready Box with a machine from `driver` that probes clean.
pub(super) fn fx_with(driver: FakeDriver) -> Fx {
    fx_box(driver, Some("ahead=0 dirty=0"))
}

/// A Ready Box of repo `o/n` with no `.moor.yaml`, with a machine from `driver`.
///
/// `boxwork` is what the probe reads back from the first Box's recorded workdir
/// (`/work/box-1`); `None` leaves it unscripted, so the probe cannot tell.
pub(super) fn fx_box(driver: FakeDriver, boxwork: Option<&str>) -> Fx {
    fx_repo(driver, boxwork, None)
}

/// Like [`fx_box`], with `yaml` as the `.moor.yaml` stored for the Box's base.
pub(super) fn fx_repo(driver: FakeDriver, boxwork: Option<&str>, yaml: Option<&str>) -> Fx {
    let driver = match boxwork {
        Some(text) => driver.peek_text(
            "box-1",
            &BoxPath::under("/work/box-1", ".moor/boxwork").unwrap(),
            text,
        ),
        None => driver,
    };
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let proc = proc();
    let cx = Cx::fake(dir.path(), driver.clone(), proc.clone());
    let id = admit(&cx, &driver, yaml);
    Fx {
        dir,
        cx,
        driver,
        proc,
        id,
    }
}

/// Admits Box `t` of repo `o/n` holding `yaml` as its `.moor.yaml`, with a Ready machine
/// acquired from `driver`.
fn admit(cx: &Cx, driver: &FakeDriver, yaml: Option<&str>) -> BoxId {
    let store = cx.store().unwrap();
    let id = store
        .apply(crate::testing::cx::new_box_of(
            BoxName::parse_allow_bare("t").unwrap(),
            crate::testing::cx::box_repo(yaml),
        ))
        .unwrap()
        .box_id;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        box_workdir: Field::Set("/work/box-1".into()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            pool_machine: None,
            provider,
            box_id: id,
            name: BoxName::parse_allow_bare("t").unwrap(),
            class: None,
            nonce: "n".to_owned(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired { handle: got.handle });
    store.apply(d).unwrap();
    id
}

pub(super) fn fx() -> Fx {
    fx_with(FakeDriver::new())
}

pub(super) fn row(f: &Fx) -> crate::store::BoxRow {
    f.cx.store().unwrap().box_row(f.id).unwrap().unwrap()
}

pub(super) fn is_working(r: &crate::store::BoxRow) -> bool {
    r.lifecycle == BoxLifecycle::Ready && !r.session.is_quiet()
}

pub(super) fn box_state(f: &Fx) -> BoxState {
    row(f).boxed.unwrap().state
}

#[test]
fn clean_close_deletes_the_box_and_closes() {
    let f = fx();
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert!(v.box_deleted);
    assert_eq!(box_state(&f), BoxState::Deleted);
    assert!(f.driver.calls().iter().any(|c| c.starts_with("release")));
    assert!(f.dir.path().is_dir());
}

#[test]
fn a_clean_close_asks_host_git_nothing_about_unsaved_work() {
    let f = fx();
    run(&f.cx, &req(false)).unwrap();
    let git: Vec<_> = f
        .proc
        .calls()
        .into_iter()
        .filter(|c| c.tool == Tool::Git)
        .map(|c| c.args)
        .collect();
    assert!(git.is_empty(), "{git:?}");
}

#[test]
fn unpushed_work_in_the_box_is_refused_without_force() {
    let f = fx_box(FakeDriver::new(), Some("ahead=2 dirty=0"));
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(is_working(&row(&f)));
    assert!(!f.driver.calls().iter().any(|c| c.starts_with("release")));
    let v = run(&f.cx, &req(true)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
}

#[test]
fn release_not_found_counts_as_deleted() {
    let f = fx();
    let h = row(&f).boxed.unwrap().handle.unwrap();
    f.driver.forget(&h);
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    assert_eq!(box_state(&f), BoxState::Deleted);
}

#[test]
fn release_error_on_a_ready_box_keeps_the_box_open() {
    let f = fx_with(FakeDriver::new().fail_release_always(DriverError::Failed {
        summary: RAW.into(),
    }));
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(
        e.error.to_string().contains(RAW),
        "the adapter summary is surfaced"
    );
    let h = row(&f).boxed.unwrap().handle.unwrap();
    let by_hand = format!(
        "release the Box by hand: stop it from /leases/{}",
        h.as_str()
    );
    assert!(
        e.hints.iter().any(|x| x.contains(&by_hand)),
        "{:?}",
        e.hints
    );
    assert!(is_working(&row(&f)));
    assert_eq!(box_state(&f), BoxState::Ready);
}

#[test]
fn release_error_on_a_failed_box_warns_and_closes() {
    let f = fx_with(FakeDriver::new().fail_release_always(DriverError::Failed {
        summary: RAW.into(),
    }));
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).box_input(BoxInput::ReleaseErr { cause: "x".into() }))
        .unwrap();
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert!(!v.box_deleted);
    assert_eq!(box_state(&f), BoxState::ReleaseFailed);
    let warnings = f.cx.take_warnings();
    assert!(!warnings.is_empty());
    assert!(warnings.iter().all(|w| !w.message.contains(RAW)));
}
