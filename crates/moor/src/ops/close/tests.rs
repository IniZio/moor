use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::seam::proc::Tool;
use moor_core::seam::surface::SurfaceError;
use moor_core::{BoxId, BoxName, BoxPath, CloseReq, ErrorKind, ProviderId};

use super::run;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

mod agent;
mod artifacts;
mod boxwork;
mod branch;
mod promote;
mod proxy;
mod reports;
mod target;
mod teardown;
mod transcript;
mod wake;
mod worktree;

const RAW: &str = "RAWDRIVER-7731";
/// The host commit id the probe compares against.
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

pub(super) struct Fx {
    pub(super) dir: tempfile::TempDir,
    pub(super) cx: Cx,
    pub(super) driver: Arc<FakeDriver>,
    pub(super) surface: Arc<FakeSurface>,
    pub(super) proc: Arc<ScriptedProc>,
    pub(super) id: BoxId,
}

pub(super) fn req(force: bool) -> CloseReq {
    CloseReq {
        target: BoxName::parse_allow_bare("t").unwrap().into(),
        force,
        trust_moor_yaml: false,
    }
}

/// Git answers: `unpushed` commits and a clean tree.
pub(super) fn proc(unpushed: u32) -> Arc<ScriptedProc> {
    let p = ScriptedProc::new();
    p.on(Tool::Git, &["rev-list"], Reply::Ok(format!("{unpushed}\n")));
    p.on(Tool::Git, &["status"], Reply::Ok(String::new()));
    p.on(Tool::Git, &["ls-tree"], Reply::Ok(String::new()));
    p.on(
        Tool::Git,
        &["rev-parse", "HEAD"],
        Reply::Ok(format!("{HEAD}\n")),
    );
    p.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok("/main/.git\n".to_owned()),
    );
    Arc::new(p)
}

/// A Ready Box with a machine from `driver` that probes clean.
pub(super) fn fx_with(driver: FakeDriver, surface: FakeSurface, unpushed: u32) -> Fx {
    fx_box(driver, surface, unpushed, Some("ahead=0 dirty=0"))
}

/// A Ready Box with a machine from `driver`; the worktree is a real directory.
///
/// `boxwork` is what the probe reads back from the first Box's recorded workdir
/// (`/work/box-1`); `None` leaves it unscripted, so the probe cannot tell.
pub(super) fn fx_box(
    driver: FakeDriver,
    surface: FakeSurface,
    unpushed: u32,
    boxwork: Option<&str>,
) -> Fx {
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
    let surface = Arc::new(surface);
    let proc = proc(unpushed);
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        driver.clone(),
        surface.clone(),
        proc.clone(),
    );
    let id = admit(&cx, &driver, dir.path());
    Fx {
        dir,
        cx,
        driver,
        surface,
        proc,
        id,
    }
}

/// Admits Box `t` with a Ready machine acquired from `driver`.
fn admit(cx: &Cx, driver: &FakeDriver, dir: &Path) -> BoxId {
    let store = cx.store().unwrap();
    let id = store
        .apply(crate::testing::cx::new_box(
            BoxName::parse_allow_bare("t").unwrap(),
        ))
        .unwrap()
        .box_id;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        worktree: Field::Set(dir.display().to_string()),
        workspace_id: Field::Set("ws-1".into()),
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
            provider,
            box_id: id,
            name: BoxName::parse_allow_bare("t").unwrap(),
            class: None,
            nonce: "n".to_owned(),
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
    fx_with(FakeDriver::new(), FakeSurface::new(PathBuf::from("/s")), 0)
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
    assert!(v.box_deleted && v.workspace_closed && !v.worktree_kept);
    assert_eq!(box_state(&f), BoxState::Deleted);
    assert!(f.driver.calls().iter().any(|c| c.starts_with("release")));
    assert!(f.dir.path().is_dir());
}

#[test]
fn unpushed_work_is_refused_without_force() {
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 2);
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
    let f = fx_with(
        FakeDriver::new().fail_release_always(DriverError::Failed {
            summary: RAW.into(),
        }),
        FakeSurface::new("/s"),
        0,
    );
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
    let f = fx_with(
        FakeDriver::new().fail_release_always(DriverError::Failed {
            summary: RAW.into(),
        }),
        FakeSurface::new("/s"),
        0,
    );
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

#[test]
fn missing_workspace_is_tolerated() {
    let f = fx_with(
        FakeDriver::new(),
        FakeSurface::new("/s").fail_close_workspace(SurfaceError::NotFound),
        0,
    );
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert!(v.workspace_closed);
    assert!(f.cx.take_warnings().is_empty());
}
