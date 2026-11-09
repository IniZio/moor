//! Closing a Box while a start holds its lock and runs its setup.

use std::sync::Arc;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::seam::clock::Ts;
use moor_core::ErrorKind;

use super::{fx_with, req, run, Fx};
use crate::cx::owners::{Cmd, Held};
use crate::cx::Cx;
use crate::env::Cancel;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;

/// A Ready Box whose setup a start is running: the setup intent is recorded and the start holds
/// the Box's turn until the returned hold drops, with `cancel` as its stop flag.
fn in_setup(cancel: &Cancel) -> (Fx, Held) {
    let f = fx_with(FakeDriver::new());
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).intent(IntentChange::Set(Intent::Setup {
            pid: std::process::id(),
        })))
        .unwrap();
    let held =
        f.cx.owners_arc()
            .hold_with(f.id, Cmd::Start, cancel.clone());
    (f, held)
}

fn lifecycle(f: &Fx) -> BoxLifecycle {
    f.cx.store()
        .unwrap()
        .box_row(f.id)
        .unwrap()
        .unwrap()
        .lifecycle
}

#[test]
fn a_plain_close_during_setup_is_refused_and_names_force() {
    let cancel = Cancel::default();
    let (f, _held) = in_setup(&cancel);
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert_eq!(e.error.to_string(), "box busy: start in progress");
    assert!(
        e.hints
            .iter()
            .any(|h| h.contains("`moor close t --force` interrupts the start")),
        "{:?}",
        e.hints
    );
    assert!(!cancel.is_cancelled());
    assert_eq!(lifecycle(&f), BoxLifecycle::Ready);
}

#[test]
fn a_forced_close_whose_start_does_not_stop_leaves_the_box_alone() {
    let cancel = Cancel::default();
    let (f, _held) = in_setup(&cancel);
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let (dir, driver, proc, at) = (
        f.dir.path().to_owned(),
        f.driver.clone(),
        f.proc.clone(),
        clock.clone(),
    );
    let owners = Arc::clone(f.cx.owners_arc());
    let closer = std::thread::spawn(move || {
        let cx =
            Cx::fake_with_clock(&dir, driver, proc, UserConfig::default(), at).with_owners(owners);
        run(&cx, &req(true))
            .map(|_| ())
            .map_err(|e| (e.kind, e.error.to_string()))
    });
    while !closer.is_finished() {
        moor_core::seam::clock::Clock::sleep(clock.as_ref(), Duration::from_secs(1));
        std::thread::sleep(Duration::from_millis(1));
    }
    let (kind, message) = closer.join().unwrap().unwrap_err();
    assert_eq!(kind, ErrorKind::Refused);
    assert_eq!(
        message,
        "box busy: the start of t did not stop within 30s of the forced close; \
         the Box was left as it is"
    );
    assert!(cancel.is_cancelled());
    assert_eq!(lifecycle(&f), BoxLifecycle::Ready);
    assert!(!f.driver.calls().iter().any(|c| c.starts_with("release ")));
}
