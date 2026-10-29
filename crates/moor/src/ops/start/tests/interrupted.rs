//! Tests for a start its process stopped running part-way through the acquire.

use std::sync::{Arc, Mutex};

use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::seam::driver::DriverError;
use moor_core::{BoxId, BoxName, CloseReq, ErrorKind};

use super::setup::start_req;
use super::{script_base_yaml, world, World};
use crate::cx::Cx;
use crate::ops::close;
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::start::run;
use crate::store::BoxRow;
use crate::testing::driver::FakeDriver;

/// A world whose first start died inside the acquire, leaving Box 1 starting with no handle.
fn cut_off(driver: FakeDriver) -> World {
    let w = world(driver);
    script_base_yaml(&w, None);
    w.driver.notes_while_acquiring(&[], || {
        std::panic::resume_unwind(Box::new("process died mid-acquire"))
    });
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(run(&w.cx, &start_req()));
    }));
    assert!(crashed.is_err());
    w.driver.notes_while_acquiring(&[], || {});
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::SettingUp);
    assert_eq!(
        row.boxed.as_ref().map(|b| b.state),
        Some(BoxState::Starting)
    );
    assert!(row.boxed.as_ref().is_some_and(|b| b.handle.is_none()));
    w
}

fn box_row(w: &World) -> BoxRow {
    w.cx.store()
        .unwrap()
        .box_row(BoxId::new(1))
        .unwrap()
        .unwrap()
}

fn releases(w: &World) -> Vec<String> {
    let calls = w.driver.calls();
    calls
        .into_iter()
        .filter(|c| c.starts_with("release "))
        .collect()
}

fn close_demo(w: &World) -> Result<moor_core::view::CloseView, crate::error::CommandError> {
    close_demo_in(&w.cx)
}

fn close_demo_in(cx: &Cx) -> Result<moor_core::view::CloseView, crate::error::CommandError> {
    let req = CloseReq {
        target: BoxName::parse_allow_bare("demo").unwrap().into(),
        force: true,
    };
    close::run(cx, &req)
}

#[test]
fn a_start_cut_off_by_a_restart_fails_and_releases_its_planned_machine() {
    let w = cut_off(FakeDriver::new());
    fail_interrupted(&w.cx, w.cx.store().unwrap()).unwrap();
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::SetupFailed);
    assert_eq!(row.intent, None);
    assert_eq!(releases(&w), ["release box-1"]);
    let events = w.cx.store().unwrap().events(row.id, 50).unwrap();
    let failed = events.iter().find(|e| e.kind == "setup_failed").unwrap();
    assert!(
        failed
            .payload
            .to_string()
            .contains("setup interrupted by a controller restart"),
        "{:?}",
        failed.payload
    );
}

#[test]
fn a_start_another_live_process_runs_is_left_alone() {
    let w = cut_off(FakeDriver::new());
    let store = w.cx.store().unwrap();
    store
        .apply(
            Decision::new(BoxId::new(1))
                .intent(IntentChange::Set(Intent::Provisioning { pid: 4242 })),
        )
        .unwrap();
    fail_interrupted(&w.cx, store).unwrap();
    assert_eq!(box_row(&w).lifecycle, BoxLifecycle::SettingUp);
    assert!(releases(&w).is_empty());
}

#[test]
fn close_of_a_box_with_no_handle_releases_the_planned_one() {
    let w = cut_off(FakeDriver::new());
    let v = close_demo(&w).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(releases(&w), ["release box-1"]);
    assert_eq!(box_row(&w).boxed.map(|b| b.state), Some(BoxState::Deleted));
}

#[test]
fn a_failed_release_of_an_unfinished_box_refuses_the_close() {
    let w = cut_off(FakeDriver::new().fail_release_always(DriverError::Failed {
        summary: "provider down".into(),
    }));
    fail_interrupted(&w.cx, w.cx.store().unwrap()).unwrap();
    let e = close_demo(&w).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().contains("provider down"), "{}", e.error);
    assert!(
        e.error.to_string().contains("the Box stays open"),
        "{}",
        e.error
    );
    assert_eq!(box_row(&w).lifecycle, BoxLifecycle::SetupFailed);
}

#[test]
fn close_while_the_acquire_runs_is_refused_and_the_start_keeps_its_machine() {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, None);
    let seen = Arc::new(Mutex::new(None));
    let (dir, driver, surface, proc, got) = (
        w.dir.path().to_owned(),
        w.driver.clone(),
        w.surface.clone(),
        w.proc.clone(),
        seen.clone(),
    );
    w.driver.notes_while_acquiring(&[], move || {
        let other = Cx::fake_with(
            &dir,
            driver.clone(),
            surface.clone(),
            proc.clone(),
            UserConfig::default(),
        );
        let e = close_demo_in(&other).unwrap_err();
        *got.lock().unwrap() = Some((e.kind, e.error.to_string()));
    });
    run(&w.cx, &start_req()).unwrap();
    let seen = seen
        .lock()
        .unwrap()
        .take()
        .expect("close ran during the acquire");
    assert_eq!(
        seen,
        (ErrorKind::Refused, "box busy: start in progress".to_owned())
    );
    assert!(releases(&w).is_empty());
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.boxed.map(|b| b.state), Some(BoxState::Ready));
}
