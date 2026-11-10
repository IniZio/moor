//! A close moor still wants: what a restart or the wanted pass does with it.

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::{Decision, Effect, EffectChange, WantChange, Wanted};
use moor_core::domain::hands::{HandsInc, HandsInput, HandsState};
use moor_core::domain::session_state::SessionInput;
use moor_core::response::CloseRefusal;
use moor_core::seam::driver::DriverError;
use moor_core::{BoxId, BoxPath, ErrorDetail, ErrorKind};

use super::boxwork::{fx_woken, put_to_sleep};
use super::{box_state, fx, fx_box, fx_with, req, row, run, Fx};
use crate::cx::owners::Cmd;
use crate::error::CommandError;
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;
use crate::store::STORE_FILE;
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

/// The kinds of Box `id`'s events, oldest first.
fn kinds(f: &Fx, id: BoxId) -> Vec<String> {
    let events = f.cx.store().unwrap().events(id, 100).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

/// The payloads of Box `id`'s `close_refused` events, oldest first.
fn refusals(f: &Fx, id: BoxId) -> Vec<serde_json::Value> {
    let events = f.cx.store().unwrap().events(id, 100).unwrap();
    let refused = events.into_iter().filter(|e| e.kind == "close_refused");
    refused.map(|e| e.payload).collect()
}

fn raise(f: &Fx, id: BoxId) {
    let raise = Decision::new(id).want(WantChange::Raise(UNFORCED));
    f.cx.store().unwrap().apply(raise).unwrap();
}

/// Box `u` beside Box `t` of `f`, Ready, whose probe reads `text`.
fn second_box(f: &Fx, text: &str) -> BoxId {
    let id = super::admit_named(&f.cx, &f.driver, None, "u");
    let path = BoxPath::under("/work/box-1", ".moor/boxwork").unwrap();
    f.driver.set_peek_text("box-2", &path, text);
    id
}

fn warned(f: &Fx) -> Vec<String> {
    let warnings = f.cx.take_warnings().into_iter();
    warnings
        .filter(|w| w.code == "close_failed")
        .map(|w| w.message)
        .collect()
}

#[test]
fn a_wanted_close_given_up_for_unsaved_work_tells_the_box_why_before_it_sleeps_again() {
    let f = fx_woken("ahead=2 dirty=1");
    put_to_sleep(&f);
    raise(&f, f.id);
    wanted_pass(&f);
    let refused = refusals(&f, f.id);
    assert_eq!(refused.len(), 1, "{refused:?}");
    let given_up = refused.first().unwrap();
    let reason = &given_up["reason"];
    assert_eq!(reason["refusal"], "unsaved", "{reason}");
    assert_eq!(reason["ahead"], 2, "{reason}");
    assert_eq!(reason["dirty"], 1, "{reason}");
    assert!(reason["branch"].is_string(), "{reason}");
    assert_eq!(
        given_up.get("slept"),
        None,
        "the re-park tells by its own event"
    );
    let kinds = kinds(&f, f.id);
    let at = |kind: &str| kinds.iter().rposition(|k| k == kind);
    assert!(at("close_refused") < at("asleep"), "{kinds:?}");
    assert_eq!(row(&f).lifecycle, BoxLifecycle::Asleep);
    assert!(!warned(&f).is_empty());
}

#[test]
fn a_wanted_close_whose_box_cannot_sleep_again_tells_the_box_and_warns_it_stays_awake() {
    let f = fx_woken("ahead=1 dirty=0");
    put_to_sleep(&f);
    raise(&f, f.id);
    f.driver.fail_next_sleep(DriverError::Failed {
        summary: "host down".into(),
    });
    wanted_pass(&f);
    assert_eq!(refusals(&f, f.id).len(), 1);
    let kinds = kinds(&f, f.id);
    let refused = kinds.iter().position(|k| k == "close_refused").unwrap();
    let after = kinds.get(refused..).unwrap_or_default();
    assert!(!after.iter().any(|k| k == "asleep"), "{kinds:?}");
    assert_ne!(row(&f).lifecycle, BoxLifecycle::Asleep);
    let warned = warned(&f);
    assert!(
        warned
            .iter()
            .any(|m| m.contains("stays awake after the close")),
        "{warned:?}"
    );
}

#[test]
fn a_wanted_close_that_fails_before_it_starts_stays_wanted_and_tells_the_box_nothing() {
    let f = fx_box(FakeDriver::new(), Some("ahead=1 dirty=0"));
    let other = second_box(&f, "ahead=0 dirty=0");
    raise(&f, f.id);
    let held = f.cx.try_turn(other, Cmd::Settle).unwrap();
    wanted_pass(&f);
    wanted_pass(&f);
    drop(held);
    assert_eq!(refusals(&f, f.id), Vec::<serde_json::Value>::new());
    assert_eq!(row(&f).wanted, Some(UNFORCED));
    assert_eq!(warned(&f).len(), 2, "each pass still warns");
}

#[test]
fn a_give_up_that_cannot_be_recorded_keeps_the_close_wanted_warns_and_the_pass_goes_on() {
    let f = fx_box(FakeDriver::new(), Some("ahead=1 dirty=0"));
    let other = second_box(&f, "ahead=1 dirty=0");
    raise(&f, f.id);
    raise(&f, other);
    let path = f.dir.path().join(STORE_FILE);
    crate::store::db::testing::refuse_events(&path, f.id, "close_refused", "disk full");
    wanted_pass(&f);
    assert_eq!(refusals(&f, other).len(), 1, "the pass went on to Box u");
    assert_eq!(refusals(&f, f.id), Vec::<serde_json::Value>::new());
    assert_eq!(
        row(&f).wanted,
        Some(UNFORCED),
        "the next pass gives it up again"
    );
    let warned = warned(&f);
    assert!(warned.iter().any(|m| m.contains("disk full")), "{warned:?}");
}

#[test]
fn a_wanted_close_given_up_after_it_closed_its_box_records_no_close_refused() {
    let f = fx();
    let held = f.cx.try_turn(f.id, Cmd::Close { force: false }).unwrap();
    let closed = Decision::new(f.id)
        .lifecycle(BoxLifecycleInput::Close { reason: None })
        .session(SessionInput::End);
    let e = crate::ops::close::wanted::closing(&f.cx, f.id, UNFORCED, (None, true), |_, _| {
        f.cx.store().unwrap().apply(closed).unwrap();
        Err(CommandError::new(ErrorKind::Failed, "a read failed"))
    })
    .unwrap_err();
    drop(held);
    assert_eq!(e.error.to_string(), "a read failed");
    assert_eq!(row(&f).lifecycle, BoxLifecycle::Closed);
    assert_eq!(row(&f).wanted, None);
    assert_eq!(refusals(&f, f.id), Vec::<serde_json::Value>::new());
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
