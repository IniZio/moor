//! A retry forking a kept checkpoint, cut off by a restart, a lost link or a forced close.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxHandle, BoxId, BoxTarget, CloseReq};

use super::{box_roots, req, row, run, saved_layer, started, world, World, TWO_LINES};
use crate::cx::Cx;
use crate::ops::close;
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::{repo_at, FakeForge};
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

type Closed = Result<BoxLifecycle, String>;

/// A Box whose setup failed its second line on a host that keeps a cut fork going, with its first
/// layer kept.
fn failed_with_a_layer() -> (World, BoxId) {
    let driver = FakeDriver::new()
        .with_checkpoints()
        .cut_acquires_go_on()
        .exec_results([Ok(ExecOutcome::Exited { code: 1 })]);
    let w = world(TWO_LINES, driver);
    let id = started(&w);
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::SetupFailed);
    saved_layer(&w, id, 1);
    (w, id)
}

/// Another context on the same state, as a second request gets.
fn other_cx(w: &World) -> Cx {
    let forge = Arc::new(FakeForge::new());
    forge.answer(
        ForgeRepo::new("o", "n"),
        Ok(repo_at("abc123", Some(TWO_LINES))),
    );
    Cx::fake_with_clock(
        w.dir.path(),
        w.driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
        UserConfig::default(),
        w.clock.clone(),
    )
    .with_forge(forge)
}

fn releases(w: &World) -> Vec<String> {
    let calls = w.driver.calls();
    calls
        .into_iter()
        .filter(|c| c.starts_with("release "))
        .collect()
}

#[test]
fn a_retry_fork_cut_by_a_restart_is_taken_over_by_the_next_process() {
    let (w, id) = failed_with_a_layer();
    w.driver
        .during_next_fork(|| std::panic::resume_unwind(Box::new("process died mid-fork")));
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(run(&w.cx, &req()));
    }));
    assert!(crashed.is_err());
    let store = w.cx.store().unwrap();
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [id]);
    resume_setup(&w.cx, store, id).unwrap();
    let after = row(&w, id);
    assert_eq!(after.lifecycle, BoxLifecycle::Ready);
    let calls = w.driver.calls();
    assert!(calls.iter().any(|c| c == "adopt"), "{calls:?}");
    let moved = after.boxed.unwrap().handle;
    assert_ne!(
        moved,
        Some(BoxHandle::new("box-1")),
        "on the forked machine"
    );
    assert_eq!(releases(&w), ["release box-1"]);
    assert_eq!(box_roots(&w, id).len(), 1);
}

#[test]
fn a_retry_fork_cut_by_a_lost_link_is_taken_over_by_the_same_process() {
    let (w, id) = failed_with_a_layer();
    w.driver.during_next_fork(|| {});
    let lost = DriverError::Transport {
        stderr_tail: "the stream ended: unexpected end of file".to_owned(),
    };
    w.driver.fork_fails_next(lost);
    run(&w.cx, &req()).unwrap();
    let after = row(&w, id);
    assert_eq!(after.lifecycle, BoxLifecycle::Ready);
    assert!(w.driver.calls().iter().any(|c| c == "adopt"));
    assert_ne!(after.boxed.unwrap().handle, Some(BoxHandle::new("box-1")));
    assert_eq!(releases(&w), ["release box-1"]);
}

#[test]
fn a_forced_close_wins_over_a_retry_fork_and_leaves_no_machine() {
    let (mut w, id) = failed_with_a_layer();
    let fresh = other_cx(&w);
    let (cx, cancel) = std::mem::replace(&mut w.cx, fresh).cancellable();
    w.cx = cx;
    let closer: Arc<Mutex<Option<JoinHandle<Closed>>>> = Arc::default();
    let (slot, other) = (closer.clone(), Mutex::new(Some(other_cx(&w))));
    w.driver.during_next_fork(move || {
        let Some(other) = other.lock().unwrap().take() else {
            return;
        };
        *slot.lock().unwrap() = Some(std::thread::spawn(move || {
            let req = CloseReq {
                target: BoxTarget::Name(super::name()),
                force: true,
            };
            close::run(&other, &req)
                .map(|v| v.boxed.lifecycle)
                .map_err(|e| e.error.to_string())
        }));
        for _ in 0..15_000 {
            if cancel.is_cancelled() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    assert!(run(&w.cx, &req()).is_err());
    let thread = closer.lock().unwrap().take();
    let outcome = thread.expect("close ran during the fork").join().unwrap();
    assert_eq!(outcome, Ok(BoxLifecycle::Closed));
    let calls = w.driver.calls();
    assert!(
        calls.iter().any(|c| c.starts_with("cancel_acquire")),
        "the host's fork is ended: {calls:?}"
    );
    let gone: Vec<_> = releases(&w);
    assert_eq!(
        gone.len(),
        2,
        "the kept machine and the fork, once each: {calls:?}"
    );
    assert!(w.driver.list_is_empty(), "{calls:?}");
    let store = w.cx.store().unwrap();
    assert!(store.live_session(id).unwrap().is_none());
}
