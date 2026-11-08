//! A forced close wins over a start still acquiring its machine, across a restart too.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::{BoxId, BoxName, CloseReq};

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
    let (slot, other) = (closer.clone(), other_cx(&w));
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
        store.live_session(BoxId::new(1)).unwrap().is_none(),
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
    let closing = IntentChange::Set(Intent::Closing {
        pid: std::process::id(),
    });
    store
        .apply(Decision::new(BoxId::new(1)).intent(closing))
        .unwrap();
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [BoxId::new(1)]);
    resume_setup(&w.cx, store, BoxId::new(1)).unwrap();
    assert_eq!(row(&w).lifecycle, BoxLifecycle::Closed);
    let calls = w.driver.calls();
    assert!(!calls.iter().any(|c| c == "adopt"), "{calls:?}");
    assert_eq!(releases(&w), 1, "{calls:?}");
}
