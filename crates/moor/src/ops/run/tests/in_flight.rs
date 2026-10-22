//! A run while its command runs in the Box, and after its leader dies with no exit code.

use std::sync::Arc;

use super::{ran_events, ready, req, Fx, LEADER_KILLED};
use crate::cx::Cx;
use crate::ops::run::run;
use crate::testing::driver::{eventually, FakeDriver};
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

/// Runs `argv` on `f` and, once run 1 has started in the Box, calls `seen` from another thread.
fn while_running<T: Send>(f: &Fx, argv: &[&str], seen: impl FnOnce() -> T + Send) -> T {
    let pid = f.box_run_dir(1).join("pid");
    std::thread::scope(|s| {
        let probe = s.spawn(|| {
            assert!(eventually(|| pid.exists()), "the run started");
            seen()
        });
        run(&f.cx, &req(argv)).unwrap();
        probe.join().unwrap()
    })
}

#[test]
fn run_is_recorded_in_flight_while_the_command_runs() {
    let f = ready();
    let id = f.id;
    let dir = f.dir.path().to_path_buf();
    let got = while_running(&f, &["sleep", "1"], || {
        let probe = Cx::fake(
            &dir,
            Arc::new(FakeDriver::new()),
            Arc::new(FakeSurface::new()),
            Arc::new(ScriptedProc::new()),
        );
        let row = probe.store().unwrap().last_run(id).unwrap().unwrap();
        (row.argv, row.finished_at.is_none())
    });
    assert_eq!(got, (vec!["sleep".to_owned(), "1".to_owned()], true));
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert!(row.finished_at.is_some());
    assert_eq!(row.exit, Some(0));
}

#[test]
fn the_follow_claim_lives_exactly_as_long_as_the_run() {
    let f = ready();
    let id = f.id;
    let dir = f.dir.path().to_path_buf();
    let live = while_running(&f, &["sleep", "1"], || {
        let probe = Cx::fake(
            &dir,
            Arc::new(FakeDriver::new()),
            Arc::new(FakeSurface::new()),
            Arc::new(ScriptedProc::new()),
        );
        let live = probe.store().unwrap().in_flight_run(id).unwrap();
        live.map(|r| r.id)
    });
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!(live, Some(row.id), "in flight while it runs");
    let follow = f
        .dir
        .path()
        .join("runs")
        .join(format!("{}.follow", row.id.get()));
    assert!(!follow.exists(), "the claim is released when the run ends");
    let store = f.cx.store().unwrap();
    assert!(store.in_flight_run(f.id).unwrap().is_none());
    assert_eq!((row.exit, row.interrupted), (Some(0), false));
}

#[test]
fn interrupted_command_is_recorded_without_an_exit() {
    let f = ready();
    let v = run(&f.cx, &req(&[LEADER_KILLED])).unwrap();
    assert!(v.interrupted && v.ran);
    assert_eq!(v.exit, None);
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert!(row.interrupted);
    assert_eq!(row.exit, None);
    assert_eq!(ran_events(&f), 1);
    let id = row.id;
    assert_eq!(f.host_log(id), f.box_log(id));
}
