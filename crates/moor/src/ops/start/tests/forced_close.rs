//! A forced close wins over a start still acquiring its machine, across a restart too.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, WantChange, Wanted};
use moor_core::domain::hands::HandsState;
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

type Closed = Result<BoxLifecycle, String>;

fn other_cx(w: &World) -> Cx {
    Cx::fake_with(
        w.dir.path(),
        w.driver.clone(),
        w.proc.clone(),
        UserConfig::default(),
    )
}

fn close_demo(cx: &Cx) -> Closed {
    let req = CloseReq {
        target: BoxName::parse_allow_bare("demo").unwrap().into(),
        force: true,
    };
    close::run(cx, &req)
        .map(|v| v.boxed.lifecycle)
        .map_err(|e| e.error.to_string())
}

fn row(w: &World) -> BoxRow {
    w.cx.store()
        .unwrap()
        .box_row(BoxId::new(1))
        .unwrap()
        .unwrap()
}

fn releases(w: &World) -> usize {
    let calls = w.driver.calls();
    calls.iter().filter(|c| c.starts_with("release ")).count()
}

/// Starts a Box on `driver` and runs a forced close while its acquire goes on; what the close came
/// to.
fn forced_mid_acquire(driver: FakeDriver) -> (World, Closed) {
    let mut w = world(driver);
    script_base_yaml(&w, None);
    let fresh = other_cx(&w);
    let (cx, cancel) = std::mem::replace(&mut w.cx, fresh).cancellable();
    w.cx = cx;
    let closer: Arc<Mutex<Option<JoinHandle<Closed>>>> = Arc::default();
    let closing = other_cx(&w).with_owners(Arc::clone(w.cx.owners_arc()));
    let (slot, other) = (closer.clone(), closing);
    let other = Mutex::new(Some(other));
    w.driver.notes_while_acquiring(&[], move || {
        let Some(other) = other.lock().unwrap().take() else {
            return;
        };
        *slot.lock().unwrap() = Some(std::thread::spawn(move || close_demo(&other)));
        for _ in 0..15_000 {
            if cancel.is_cancelled() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let start = run(&w.cx, &start_req());
    assert!(start.is_err(), "the start lost to the close");
    let thread = closer.lock().unwrap().take();
    let outcome = thread
        .expect("close ran during the acquire")
        .join()
        .unwrap();
    (w, outcome)
}

#[test]
fn a_forced_close_ends_an_acquire_on_a_provider_that_stops_with_its_caller() {
    let (w, closed) = forced_mid_acquire(FakeDriver::new());
    assert_eq!(closed, Ok(BoxLifecycle::Closed));
    assert_eq!(releases(&w), 1, "{:?}", w.driver.calls());
    assert_eq!(row(&w).lifecycle, BoxLifecycle::Closed);
    assert!(w.driver.list_is_empty());
    let store = w.cx.store().unwrap();
    assert!(
        store.live_launch(BoxId::new(1)).unwrap().is_none(),
        "no agent"
    );
}

#[test]
fn a_forced_close_cancels_a_boot_going_on_at_a_pool_host() {
    let (w, closed) = forced_mid_acquire(FakeDriver::new().cut_acquires_go_on());
    assert_eq!(closed, Ok(BoxLifecycle::Closed));
    let calls = w.driver.calls();
    assert!(
        calls.iter().any(|c| c.starts_with("cancel_acquire")),
        "{calls:?}"
    );
    assert_eq!(releases(&w), 1, "{calls:?}");
    assert!(w.driver.list_is_empty());
}

#[test]
fn the_next_process_honours_a_forced_close_a_restart_cut_off() {
    let w = world(FakeDriver::new().cut_acquires_go_on());
    script_base_yaml(&w, None);
    w.driver.notes_while_acquiring(&[], || {
        std::panic::resume_unwind(Box::new("process died mid-acquire"))
    });
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(run(&w.cx, &start_req()));
    }));
    assert!(crashed.is_err());
    w.driver.notes_while_acquiring(&[], || {});
    let store = w.cx.store().unwrap();
    let closing = WantChange::Raise(Wanted::Close {
        force: true,
        reason: None,
    });
    store
        .apply(Decision::new(BoxId::new(1)).want(closing))
        .unwrap();
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [BoxId::new(1)]);
    resume_setup(&w.cx, store, BoxId::new(1)).unwrap();
    assert_eq!(row(&w).lifecycle, BoxLifecycle::Closed);
    let calls = w.driver.calls();
    assert!(!calls.iter().any(|c| c == "adopt"), "{calls:?}");
    assert_eq!(releases(&w), 1, "{calls:?}");
}

#[test]
fn an_unforced_close_during_the_acquire_is_refused_and_the_start_keeps_its_machine() {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, None);
    let seen = Arc::new(Mutex::new(None));
    let owners = Arc::clone(w.cx.owners_arc());
    let (dir, driver, proc, got) = (
        w.dir.path().to_owned(),
        w.driver.clone(),
        w.proc.clone(),
        seen.clone(),
    );
    w.driver.notes_while_acquiring(&[], move || {
        let other = Cx::fake_with(&dir, driver.clone(), proc.clone(), UserConfig::default())
            .with_owners(Arc::clone(&owners));
        let req = CloseReq {
            target: BoxName::parse_allow_bare("demo").unwrap().into(),
            force: false,
        };
        let closing = std::thread::spawn(move || close::run(&other, &req).map(drop));
        let e = closing.join().unwrap().unwrap_err();
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
    assert_eq!(releases(&w), 0);
    let row = row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.hands.map(|b| b.state), Some(HandsState::Ready));
}
