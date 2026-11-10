//! A close moor still wants: what a restart or the wanted pass does with it.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, Effect, EffectChange, WantChange, Wanted};
use moor_core::domain::hands::{HandsInc, HandsInput, HandsState};
use moor_core::response::CloseRefusal;
use moor_core::seam::driver::DriverError;
use moor_core::{ErrorDetail, ErrorKind};

use super::boxwork::{fx_woken, put_to_sleep};
use super::{box_state, fx_box, fx_with, req, row, run, Fx};
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;
use crate::testing::driver::FakeDriver;

const UNFORCED: Wanted = Wanted::Close {
    force: false,
    reason: None,
};

fn releases(f: &Fx) -> usize {
    let calls = f.driver.calls();
    calls.iter().filter(|c| c.starts_with("release ")).count()
}

fn unsaved(e: &crate::error::CommandError) -> bool {
    matches!(
        e.detail.as_deref(),
        Some(ErrorDetail::CloseRefused(CloseRefusal::Unsaved { .. }))
    )
}

fn wanted_pass(f: &Fx) {
    crate::ops::wanted_pass(&f.cx, f.cx.store().unwrap(), &|| false).unwrap();
}

/// Box `f` as a restart finds it after cutting an unforced close mid-release.
fn cut_mid_release(f: &Fx) {
    let store = f.cx.store().unwrap();
    let cut = Decision::new(f.id)
        .want(WantChange::Raise(UNFORCED))
        .effect(EffectChange::Next(Effect::Release));
    store.apply(cut).unwrap();
    assert_eq!(fail_interrupted(&f.cx, store).unwrap(), [f.id]);
}

fn assert_closed_once(f: &Fx) {
    let r = row(f);
    assert_eq!(r.lifecycle, BoxLifecycle::Closed);
    assert_eq!(r.effect, None);
    assert_eq!(r.wanted, None);
    assert_eq!(releases(f), 1);
}

#[test]
fn an_unforced_close_a_restart_cut_mid_release_finishes_though_the_box_cannot_be_read() {
    let f = fx_box(FakeDriver::new(), None);
    cut_mid_release(&f);
    resume_setup(&f.cx, f.cx.store().unwrap(), f.id).unwrap();
    assert_closed_once(&f);
}

#[test]
fn an_unforced_close_a_restart_cut_mid_release_finishes_without_checking_for_unsaved_work() {
    let f = fx_box(FakeDriver::new(), Some("ahead=1 dirty=0"));
    cut_mid_release(&f);
    resume_setup(&f.cx, f.cx.store().unwrap(), f.id).unwrap();
    assert_closed_once(&f);
}

#[test]
fn an_unforced_close_a_restart_cut_mid_release_finishes_once_its_machine_is_gone() {
    let f = fx_box(FakeDriver::new(), None);
    let gone = Decision::new(f.id).hands_input(HandsInc::FIRST, HandsInput::DriverMissing);
    f.cx.store().unwrap().apply(gone).unwrap();
    cut_mid_release(&f);
    resume_setup(&f.cx, f.cx.store().unwrap(), f.id).unwrap();
    let r = row(&f);
    assert_eq!(r.lifecycle, BoxLifecycle::Closed);
    assert_eq!(r.effect, None);
    assert_eq!(r.wanted, None);
}

#[test]
fn an_unforced_close_a_restart_cut_before_its_release_is_still_refused_for_unsaved_work() {
    let f = fx_box(FakeDriver::new(), Some("ahead=1 dirty=0"));
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).want(WantChange::Raise(UNFORCED)))
        .unwrap();
    assert_eq!(fail_interrupted(&f.cx, store).unwrap(), [f.id]);
    let e = resume_setup(&f.cx, store, f.id).unwrap_err();
    assert!(unsaved(&e), "{}", e.error);
    assert_ne!(row(&f).lifecycle, BoxLifecycle::Closed);
    assert_eq!(row(&f).wanted, None);
    assert_eq!(releases(&f), 0);
}

#[test]
fn a_close_refused_for_unsaved_work_is_no_longer_wanted_and_no_pass_closes_the_box() {
    let f = fx_box(FakeDriver::new(), Some("ahead=1 dirty=0"));
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert!(unsaved(&e), "{}", e.error);
    assert_eq!(row(&f).wanted, None);
    wanted_pass(&f);
    assert_ne!(row(&f).lifecycle, BoxLifecycle::Closed);
    assert_eq!(releases(&f), 0);
}

#[test]
fn a_wanted_close_that_wakes_its_box_to_unsaved_work_warns_and_parks_the_box_again() {
    let f = fx_woken("ahead=2 dirty=0");
    put_to_sleep(&f);
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).want(WantChange::Raise(UNFORCED)))
        .unwrap();
    wanted_pass(&f);
    let warned = f.cx.take_warnings();
    let said: Vec<_> = warned
        .iter()
        .filter(|w| w.code == "close_failed")
        .map(|w| w.message.as_str())
        .collect();
    assert!(
        said.iter().any(|m| m.contains("the Box holds work")),
        "{warned:?}"
    );
    assert_eq!(row(&f).wanted, None);
    assert_eq!(row(&f).lifecycle, BoxLifecycle::Asleep);
    assert_eq!(box_state(&f), HandsState::Asleep);
    assert_eq!(releases(&f), 0);
}

#[test]
fn a_close_cut_mid_release_leaves_the_release_in_flight_and_a_restart_finishes_it() {
    let f = fx_with(FakeDriver::new());
    f.driver.on_release(|| {
        std::panic::resume_unwind(Box::new("process died mid-release"));
    });
    let cut = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(run(&f.cx, &req(true)));
    }));
    assert!(cut.is_err());
    assert_eq!(row(&f).effect, Some(Effect::Release));
    f.driver.on_release(|| {});
    let store = f.cx.store().unwrap();
    assert_eq!(fail_interrupted(&f.cx, store).unwrap(), [f.id]);
    resume_setup(&f.cx, store, f.id).unwrap();
    assert_eq!(row(&f).lifecycle, BoxLifecycle::Closed);
    assert_eq!(row(&f).effect, None);
}

#[test]
fn a_release_that_fails_leaves_a_run_in_flight_its_effect() {
    let f = fx_with(FakeDriver::new().fail_release_always(DriverError::Failed {
        summary: "provider down".into(),
    }));
    let store = f.cx.store().unwrap();
    let (held, _claim) = store
        .begin_run(crate::store::NewRun {
            box_id: f.id,
            argv: vec!["make".to_owned()],
            record: None,
            sha: None,
            pid: 1,
        })
        .unwrap();
    let e = run(&f.cx, &req(true)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed, "{}", e.error);
    assert_eq!(row(&f).effect, Some(Effect::Run { run: held }));
}
