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
/// request once the resume has settled it; returns how the steer ended.
fn resume_and_steer(w: &World) -> Result<(), CommandError> {
    let owners = Arc::new(Owners::new(Arc::new(SystemClock::real())));
    let (steered, steer_ended) = mpsc::channel();
    let got = std::cell::Cell::new(None);
    let lines = Moor::from_cx(restarted(w, &owners)).resume(BoxId::new(1), |_| {
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
    got.into_inner().expect("the resume settled the Box")
}

/// Asserts `got` is the refusal a steer gets while a start sets the Box up.
fn refused_for_setup(got: Result<(), CommandError>) {
    let e = got.expect_err("the steer was refused");
    assert_eq!(e.kind, ErrorKind::Refused);
    assert_eq!(e.error.to_string(), "box busy: setup in progress");
}

#[test]
fn a_restart_mid_setup_follows_the_setup_as_the_owners_first_turn_and_refuses_a_steer_meanwhile() {
    let (w, attached) = restarted_mid_setup("setup: [a, b]\n");
    refused_for_setup(resume_and_steer(&w));
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.intent, None);
    assert_eq!(setup_attaches(&w), [attached.clone(), attached]);
}

#[test]
fn a_restart_mid_acquire_acquires_exactly_once_more_and_refuses_a_steer_meanwhile() {
    let w = cut_off(FakeDriver::new());
    let before = w.driver.acquired_nonces().len();
    refused_for_setup(resume_and_steer(&w));
    assert_eq!(w.driver.acquired_nonces().len(), before + 1);
    assert_eq!(releases(&w), ["release box-1"]);
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.intent, None);
}
