//! A controller that restarts resumes each Box it finds mid-start as that Box owner's first turn.

use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::{BoxId, BoxName, ErrorKind, SteerReq};

use super::interrupted::{box_row, cut_off, releases, restarted_mid_setup, setup_attaches};
use super::World;
use crate::adapters::clock::SystemClock;
use crate::cx::owners::Owners;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;
use crate::ops::steer;
use crate::testing::driver::FakeDriver;
use crate::Moor;

/// A context of the next controller process over `w`'s store, admitted through `owners`.
fn restarted(w: &World, owners: &Arc<Owners>) -> Cx {
    Cx::fake_with(
        w.dir.path(),
        w.driver.clone(),
        w.proc.clone(),
        UserConfig::default(),
    )
    .with_forge(w.forge.clone())
    .with_owners(Arc::clone(owners))
}

/// Resumes Box 1 in a new controller over `w`'s store. A steer is fired at the Box from another
/// request once the resume has settled it, which is when the controller starts serving; returns
/// how the steer ended, and the setup attaches and acquires made by then.
fn resume_and_steer(w: &World) -> (Result<(), CommandError>, (usize, usize)) {
    let owners = Arc::new(Owners::new(Arc::new(SystemClock::real())));
    let (steered, steer_ended) = mpsc::channel();
    let got = std::cell::Cell::new(None);
    let at_settle = std::cell::Cell::new((0, 0));
    let lines = Moor::from_cx(restarted(w, &owners)).resume(BoxId::new(1), |_| {
        at_settle.set((setup_attaches(w).len(), w.driver.acquired_nonces().len()));
        let request = restarted(w, &owners);
        thread::spawn(move || {
            let req = SteerReq {
                target: BoxName::parse_allow_bare("demo").unwrap().into(),
                text: Some("hi".to_owned()),
                menu: None,
                interrupt: false,
            };
            steered.send(steer::run(&request, &req).map(drop)).ok();
        });
        // The resume keeps the turn until the steer answers, so a queued steer would never end.
        let answered = steer_ended.recv_timeout(Duration::from_secs(10));
        got.set(Some(answered.expect(
            "the steer was answered while the resume held the turn",
        )));
    });
    assert!(lines.is_empty(), "{lines:?}");
    let got = got.into_inner().expect("the resume settled the Box");
    (got, at_settle.into_inner())
}

/// Asserts `got` is the refusal a steer gets while the resume holds the Box.
fn refused_for_setup(got: Result<(), CommandError>) {
    let e = got.expect_err("the steer was refused");
    assert_eq!(e.kind, ErrorKind::Refused);
    assert_eq!(e.error.to_string(), "box busy: resume in progress");
}

#[test]
fn a_restart_mid_setup_follows_the_setup_as_the_owners_first_turn_and_refuses_a_steer_meanwhile() {
    let (w, attached) = restarted_mid_setup("setup: [a, b]\n");
    let (steered, (followed_at_settle, _)) = resume_and_steer(&w);
    refused_for_setup(steered);
    assert_eq!(
        followed_at_settle, 1,
        "serving starts before the setup is followed again"
    );
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.intent, None);
    assert_eq!(setup_attaches(&w), [attached.clone(), attached]);
}

#[test]
fn a_restart_mid_acquire_acquires_exactly_once_more_and_refuses_a_steer_meanwhile() {
    let w = cut_off(FakeDriver::new());
    let before = w.driver.acquired_nonces().len();
    let (steered, (_, acquires)) = resume_and_steer(&w);
    refused_for_setup(steered);
    assert_eq!(
        acquires, before,
        "serving starts before the acquire runs again"
    );
    assert_eq!(w.driver.acquired_nonces().len(), before + 1);
    assert_eq!(releases(&w), ["release box-1"]);
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.intent, None);
}

#[test]
fn a_release_the_provider_holds_up_does_not_hold_up_serving() {
    let w = cut_off(FakeDriver::new());
    let before = w.driver.acquired_nonces().len();
    let (entered, in_release) = mpsc::channel();
    let (unblock, blocked) = mpsc::channel::<()>();
    let gate = std::sync::Mutex::new(Some((entered, blocked)));
    w.driver.on_release(move || {
        let first = gate.lock().unwrap().take();
        if let Some((entered, blocked)) = first {
            entered.send(()).unwrap();
            blocked.recv().ok();
        }
    });
    let owners = Arc::new(Owners::new(Arc::new(SystemClock::real())));
    let (boot, request) = (restarted(&w, &owners), restarted(&w, &owners));
    let (serving, served) = mpsc::channel();
    let resumed = thread::spawn(move || {
        Moor::from_cx(boot).resume(BoxId::new(1), move |_| serving.send(()).unwrap())
    });
    served
        .recv_timeout(Duration::from_secs(10))
        .expect("the controller serves before the release ends");
    in_release
        .recv_timeout(Duration::from_secs(10))
        .expect("the release began");
    let req = SteerReq {
        target: BoxName::parse_allow_bare("demo").unwrap().into(),
        text: Some("hi".to_owned()),
        menu: None,
        interrupt: false,
    };
    refused_for_setup(steer::run(&request, &req).map(drop));
    unblock.send(()).unwrap();
    assert!(resumed.join().unwrap().is_empty());
    assert_eq!(w.driver.acquired_nonces().len(), before + 1);
    assert_eq!(box_row(&w).lifecycle, BoxLifecycle::Ready);
}

/// The kinds of the events Box 1 has, oldest first.
fn event_kinds(w: &World) -> Vec<String> {
    let store = w.cx.store().unwrap();
    let events = store.events(BoxId::new(1), usize::MAX).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

#[test]
fn a_restart_mid_setup_that_resumes_emits_no_box_failed() {
    let (w, _) = restarted_mid_setup("setup: [a, b]\n");
    let (steered, _) = resume_and_steer(&w);
    refused_for_setup(steered);
    let kinds = event_kinds(&w);
    assert!(!kinds.iter().any(|k| k == "box_failed"), "{kinds:?}");
    assert!(!kinds.iter().any(|k| k == "setup_failed"), "{kinds:?}");
}

#[test]
fn a_restart_mid_acquire_that_acquires_again_emits_no_box_failed() {
    let w = cut_off(FakeDriver::new());
    let (steered, _) = resume_and_steer(&w);
    refused_for_setup(steered);
    let kinds = event_kinds(&w);
    assert!(!kinds.iter().any(|k| k == "box_failed"), "{kinds:?}");
    assert!(!kinds.iter().any(|k| k == "setup_failed"), "{kinds:?}");
    assert_eq!(box_row(&w).lifecycle, BoxLifecycle::Ready);
}

#[test]
fn a_second_restart_mid_acquire_cannot_resume_so_the_box_fails() {
    let w = cut_off(FakeDriver::new());
    let store = w.cx.store().unwrap();
    fail_interrupted(&w.cx, store).unwrap();
    w.driver.notes_while_acquiring(&[], || {
        std::panic::resume_unwind(Box::new("process died mid-acquire"))
    });
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(resume_setup(&w.cx, store, BoxId::new(1)));
    }));
    assert!(crashed.is_err());
    w.driver.notes_while_acquiring(&[], || {});
    assert!(fail_interrupted(&w.cx, store).unwrap().is_empty());
    let kinds = event_kinds(&w);
    assert!(kinds.iter().any(|k| k == "box_failed"), "{kinds:?}");
    assert!(kinds.iter().any(|k| k == "setup_failed"), "{kinds:?}");
}
