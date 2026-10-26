use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::seam::driver::DriverError;
use moor_core::ErrorKind;

use super::teardown::{exec_calls, fx_yaml, YAML};
use super::{box_state, is_working, req, row, run, RAW};
use crate::testing::driver::FakeDriver;

/// A Box whose machine the provider no longer lists, and whose release fails the way crabbox
/// fails to stop a vanished machine.
fn vanished() -> super::Fx {
    vanished_with(FakeDriver::new())
}

fn vanished_with(driver: FakeDriver) -> super::Fx {
    let f = fx_yaml(
        driver.fail_release_always(DriverError::Failed {
            summary: RAW.into(),
        }),
        YAML,
    );
    f.driver.forget(&row(&f).boxed.unwrap().handle.unwrap());
    f
}

#[test]
fn force_closes_a_box_whose_machine_vanished_in_one_call() {
    let f = vanished();
    let v = run(&f.cx, &req(true)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert!(v.workspace_closed);
    assert_eq!(box_state(&f), BoxState::Gone);
    let warnings = f.cx.take_warnings();
    let codes: Vec<_> = warnings.iter().map(|w| w.code.as_str()).collect();
    assert_eq!(codes, ["box_already_gone"], "one note: {warnings:?}");
    assert!(warnings.iter().all(|w| w.message.contains("already gone")));
    assert_eq!(exec_calls(&f), 0, "nothing runs on a machine that is gone");
    assert!(!f.driver.calls().iter().any(|c| c.starts_with("release")));
}

#[test]
fn force_close_of_a_listed_box_still_fails_on_a_release_error() {
    let f = fx_yaml(
        FakeDriver::new().fail_release_always(DriverError::Failed {
            summary: RAW.into(),
        }),
        YAML,
    );
    let e = run(&f.cx, &req(true)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().contains(RAW));
    assert_eq!(box_state(&f), BoxState::Ready);
    assert_ne!(row(&f).lifecycle, BoxLifecycle::Closed);
}

#[test]
fn force_close_goes_on_as_usual_when_the_listing_fails() {
    let f = vanished_with(FakeDriver::new().fail_list(DriverError::Failed {
        summary: "list down".into(),
    }));
    let e = run(&f.cx, &req(true)).unwrap_err();
    assert!(e.error.to_string().contains(RAW), "{e:?}");
    assert_eq!(box_state(&f), BoxState::Ready);
}

#[test]
fn close_without_force_does_not_consult_the_listing() {
    let f = vanished();
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert!(!f.driver.calls().iter().any(|c| c == "list"), "{e:?}");
    assert!(is_working(&row(&f)));
}
