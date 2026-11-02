use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::seam::driver::DriverError;

use super::boxwork::fx_setup_failed;
use super::{fx, fx_with, req, run, Fx, RAW};
use crate::store::lease::LeaseKind;
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

fn plan_leases(f: &Fx) {
    let store = f.cx.store().unwrap();
    let root = store
        .plan_lease(f.id, LeaseKind::BoxRoot, "/leases/box-1")
        .unwrap();
    store
        .plan_lease(f.id, LeaseKind::BoxDir, "/work/box-1")
        .unwrap();
    store.hold_lease(root).unwrap();
}

fn lease_count(f: &Fx) -> usize {
    f.cx.store().unwrap().leases(f.id).unwrap().len()
}

#[test]
fn closing_a_setup_failed_box_drops_its_leases() {
    let f = fx_setup_failed();
    plan_leases(&f);
    assert_eq!(lease_count(&f), 2);
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(lease_count(&f), 0);
}

#[test]
fn closing_a_ready_box_drops_its_leases() {
    let f = fx();
    plan_leases(&f);
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(lease_count(&f), 0);
}

#[test]
fn a_release_failed_box_keeps_its_leases_for_the_manual_release() {
    let f = fx_with(
        FakeDriver::new().fail_release_always(DriverError::Failed {
            summary: RAW.into(),
        }),
        FakeSurface::new(),
    );
    plan_leases(&f);
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).box_input(BoxInput::ReleaseErr { cause: "x".into() }))
        .unwrap();
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(lease_count(&f), 2);
}

#[test]
fn a_box_that_stays_open_keeps_its_leases() {
    let f = fx_with(
        FakeDriver::new().fail_release_always(DriverError::Failed {
            summary: RAW.into(),
        }),
        FakeSurface::new(),
    );
    plan_leases(&f);
    run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(lease_count(&f), 2);
}
