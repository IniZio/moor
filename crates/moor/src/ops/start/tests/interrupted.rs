//! Tests for a start its process stopped running part-way through the acquire.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, Field, Fields, Intent, IntentChange};
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::{BoxId, BoxName, CloseReq, ErrorKind};

use super::setup::start_req;
use super::{script_base_yaml, world, World};
use crate::cx::Cx;
use crate::ops::close;
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;
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

type Closed = Result<BoxLifecycle, String>;

#[test]
fn a_forced_close_during_setup_interrupts_the_start_and_closes_the_box() {
    let mut w = world(FakeDriver::new().exec_ends_when("sdkmanager", ExecOutcome::Interrupted));
    script_base_yaml(&w, Some("setup: [sdkmanager --install ndk]\n"));
    let fresh = Cx::fake(
        w.dir.path(),
        w.driver.clone(),
        w.surface.clone(),
        w.proc.clone(),
    );
    let (cx, cancel) = std::mem::replace(&mut w.cx, fresh).cancellable();
    w.cx = cx;
    let closer: Arc<Mutex<Option<std::thread::JoinHandle<Closed>>>> = Arc::default();
    let (dir, driver, surface, proc, slot, stop) = (
        w.dir.path().to_owned(),
        w.driver.clone(),
        w.surface.clone(),
        w.proc.clone(),
        closer.clone(),
        cancel,
    );
    w.driver.on_exec(move || {
        let mut slot = slot.lock().unwrap();
        if slot.is_none() {
            let (dir, driver, surface, proc) =
                (dir.clone(), driver.clone(), surface.clone(), proc.clone());
            *slot = Some(std::thread::spawn(move || {
                let other = Cx::fake_with(&dir, driver, surface, proc, UserConfig::default());
                close_demo_in(&other)
                    .map(|v| v.boxed.lifecycle)
                    .map_err(|e| e.error.to_string())
            }));
        }
        drop(slot);
        while !stop.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    let e = run(&w.cx, &start_req()).unwrap_err();
    assert_eq!(e.error.to_string(), "setup interrupted by a forced close");
    let closing = closer.lock().unwrap().take();
    let outcome = closing.expect("close ran during the setup").join().unwrap();
    assert_eq!(outcome, Ok(BoxLifecycle::Closed));
    let events = w.cx.store().unwrap().events(BoxId::new(1), 50).unwrap();
    let at = |kind: &str| events.iter().find(|e| e.kind == kind).map(|e| e.id);
    let failed = events.iter().find(|e| e.kind == "setup_failed").unwrap();
    assert_eq!(
        failed.payload.get("line"),
        Some(&serde_json::json!("setup interrupted by a forced close"))
    );
    assert!(at("setup_failed") < at("closed"), "{events:?}");
    assert_eq!(box_row(&w).lifecycle, BoxLifecycle::Closed);
}

/// The argv of every exec that attaches to a detached setup run.
fn setup_attaches(w: &World) -> Vec<Vec<String>> {
    w.driver
        .execs()
        .into_iter()
        .map(|(_, spec, _)| spec.argv)
        .filter(|argv| argv.last().is_some_and(|a| a.contains(".moor/runs/")))
        .collect()
}

/// A world whose first start died while it followed its setup run, as a controller restart
/// leaves it; gives back the argv of that attach.
fn restarted_mid_setup(yaml: &str) -> (World, Vec<String>) {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, Some(yaml));
    let dying = Arc::new(AtomicBool::new(true));
    let (driver, still) = (w.driver.clone(), dying.clone());
    w.driver.after_exec(move || {
        let attached = driver.execs().last().is_some_and(|(_, spec, _)| {
            spec.argv.last().is_some_and(|a| a.contains(".moor/runs/"))
        });
        if attached && still.load(Ordering::SeqCst) {
            std::panic::resume_unwind(Box::new("controller restarted mid-setup"));
        }
    });
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(run(&w.cx, &start_req()));
    }));
    assert!(crashed.is_err());
    dying.store(false, Ordering::SeqCst);
    let [attached] = setup_attaches(&w).try_into().unwrap();
    assert_eq!(box_row(&w).lifecycle, BoxLifecycle::SettingUp);
    (w, attached)
}

#[test]
fn a_restart_mid_setup_follows_the_same_run_again_and_the_box_comes_up() {
    let (w, attached) = restarted_mid_setup("setup: [a, b]\n");
    let store = w.cx.store().unwrap();
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [BoxId::new(1)]);
    assert_eq!(box_row(&w).lifecycle, BoxLifecycle::SettingUp);
    resume_setup(&w.cx, store, BoxId::new(1)).unwrap();
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.intent, None);
    assert_eq!(setup_attaches(&w), [attached.clone(), attached]);
}

#[test]
fn a_setup_followed_again_after_a_restart_records_the_line_that_failed() {
    let (w, _) = restarted_mid_setup("setup: [a, b, c]\n");
    let store = w.cx.store().unwrap();
    fail_interrupted(&w.cx, store).unwrap();
    w.driver.set_copy_text("moor setup: $ a\nmoor setup: $ b\n");
    w.driver.push_exec_result(Ok(ExecOutcome::Exited { code: 0 }));
    w.driver.push_exec_result(Ok(ExecOutcome::Exited { code: 1 }));
    resume_setup(&w.cx, store, BoxId::new(1)).unwrap();
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::SetupFailed);
    let events = store.events(row.id, 50).unwrap();
    let failed = events.iter().rfind(|e| e.kind == "setup_failed").unwrap();
    assert!(failed.payload.to_string().contains("\"b\""), "{:?}", failed.payload);
}

#[test]
fn a_restart_once_the_setup_lines_passed_leaves_only_the_later_steps_pending() {
    let (w, _) = restarted_mid_setup("setup: [a]\n");
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(BoxId::new(1)).set(Fields {
            steps_pending_at: Field::Set(1),
            ..Fields::default()
        }))
        .unwrap();
    assert!(fail_interrupted(&w.cx, store).unwrap().is_empty());
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert!(row.steps_pending_at.is_some());
}
