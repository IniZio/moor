//! A run detached in the Box, on the fake that plays the Box's run scripts with the local sh.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use moor_core::seam::clock::{Clock, Ts};

use super::{ready_with, req, Fx};
use crate::ops::run::{run, streamed};
use crate::testing::clock::FakeClock;
use crate::testing::driver::{eventually, live_in_group, FakeDriver};

fn local() -> Fx {
    ready_with(FakeDriver::new())
}

/// The pid of the run group leader whose Box run dir is `dir`, once the run has started.
fn leader(dir: &Path) -> u32 {
    let pid = dir.join("pid");
    assert!(eventually(|| pid.exists()), "the run started");
    std::fs::read_to_string(pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// Runs `work` on `f` on another thread; fails the test if it has not returned in 60 s.
fn bounded<T: Send + 'static>(f: Fx, work: impl FnOnce(&Fx) -> T + Send + 'static) -> (T, Fx) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let got = work(&f);
        let _ = tx.send((got, f));
    });
    rx.recv_timeout(Duration::from_secs(60))
        .expect("the run returned within 60 s")
}

/// Whether process `pid` is running: it exists and has not exited.
fn running(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state != "Z" && state != "X")
    })
}

/// Prints binary bytes and 3000 lines, then exits 7.
const NOISY: &str = r#"printf 'bin\000\377\200 end\n'; i=0; while [ $i -lt 3000 ]; do echo "line $i"; i=$((i+1)); done; exit 7"#;

#[test]
fn the_host_log_is_the_box_log_byte_for_byte_and_the_exit_is_recorded() {
    let f = local();
    let v = run(&f.cx, &req(&[NOISY])).unwrap();
    let id = v.run.unwrap();
    assert_eq!((v.exit, v.interrupted), (Some(7), false));
    let host = f.host_log(id);
    assert!(
        host.starts_with(b"bin\0\xff\x80 end\n"),
        "{:?}",
        host.get(..12)
    );
    assert_eq!(host, f.box_log(id));
    let row = f.cx.store().unwrap().run(id).unwrap().unwrap();
    assert_eq!((row.exit, row.interrupted), (Some(7), false));
    assert_eq!(
        row.pid,
        leader(&f.box_run_dir(id.get())),
        "runs.pid is the Box pid"
    );
    let calls = f.driver.calls();
    assert!(calls.iter().all(|c| !c.starts_with("exec")), "{calls:?}");
}

#[test]
fn a_cut_follow_resumes_from_the_offset_it_reached() {
    let f = local();
    f.driver.cut_next_follow(1000);
    let v = run(&f.cx, &req(&[NOISY])).unwrap();
    let id = v.run.unwrap();
    assert_eq!(v.exit, Some(7));
    assert_eq!(f.host_log(id), f.box_log(id));
    let follows: Vec<String> = f
        .driver
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("follow_run"))
        .collect();
    let want = format!("follow_run box-1 {}", id.get());
    assert_eq!(follows, [format!("{want} 0"), format!("{want} 1000")]);
}

#[test]
fn a_passed_deadline_cancels_the_run_in_the_box_and_is_recorded() {
    let mut f = local();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    f.cx = f.cx.with_clock(clock.clone());
    let dir = f.box_run_dir(1);
    let driver = f.driver.clone();
    let pid = thread::scope(|s| {
        let late = s.spawn(|| {
            let pid = leader(&dir);
            clock.sleep(Duration::from_hours(24) - Duration::from_secs(60));
            thread::sleep(Duration::from_millis(500));
            assert!(
                live_in_group(pid) > 0,
                "the run lives until its 24 h deadline"
            );
            assert!(!driver.calls().iter().any(|c| c.starts_with("cancel_run")));
            clock.sleep(Duration::from_secs(120));
            pid
        });
        let e = run(&f.cx, &req(&["echo started; sleep 60"])).unwrap_err();
        assert!(e.error.to_string().contains("deadline"), "{e:?}");
        late.join().unwrap()
    });
    assert_eq!(live_in_group(pid), 0, "the run's group is gone");
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert!(row.interrupted && row.finished_at.is_some(), "{row:?}");
    assert!(f.driver.calls().contains(&"cancel_run box-1 1".to_owned()));
}

#[test]
fn a_client_hangup_cancels_the_run_in_the_box() {
    let mut f = local();
    let cancel;
    (f.cx, cancel) = f.cx.cancellable();
    let dir = f.box_run_dir(1);
    let pid = thread::scope(|s| {
        let gone = s.spawn(|| {
            let pid = leader(&dir);
            cancel.cancel();
            pid
        });
        let v = run(&f.cx, &req(&["echo started; sleep 60 & sleep 60"])).unwrap();
        assert_eq!((v.exit, v.interrupted), (None, true));
        gone.join().unwrap()
    });
    assert_eq!(live_in_group(pid), 0, "the run's group is gone");
    assert!(f.driver.calls().contains(&"cancel_run box-1 1".to_owned()));
}

#[test]
fn a_panic_while_streaming_the_log_releases_the_claim_and_the_box() {
    let f = local();
    let once = AtomicBool::new(false);
    f.driver.after_follow(move || {
        if !once.swap(true, Ordering::SeqCst) {
            std::panic::resume_unwind(Box::new("the copy broke"));
        }
    });
    let (crashed, f) = bounded(f, |f| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut sink = |_: &[u8]| Ok(());
            drop(streamed(&f.cx, &req(&["echo first"]), Some(&mut sink)));
        }))
    });
    assert!(crashed.is_err(), "the panic reached the caller");
    let store = f.cx.store().unwrap();
    assert!(
        store.in_flight_run(f.id).unwrap().is_none(),
        "no run in flight"
    );
    let first = store.last_run(f.id).unwrap().unwrap();
    assert!(
        first.finished_at.is_some() && first.interrupted,
        "{first:?}"
    );
    let v = run(&f.cx, &req(&["echo second"])).unwrap();
    assert_eq!(v.exit, Some(0), "the Box lock was released");
}

#[test]
fn a_hangup_after_the_command_exited_records_its_exit_and_cancels_nothing() {
    let mut f = local();
    let cancel;
    (f.cx, cancel) = f.cx.cancellable();
    f.driver.after_follow(move || {
        cancel.cancel();
        thread::sleep(Duration::from_millis(500));
    });
    let v = run(&f.cx, &req(&["echo done; exit 0"])).unwrap();
    assert_eq!((v.exit, v.interrupted), (Some(0), false));
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!((row.exit, row.interrupted), (Some(0), false));
    let calls = f.driver.calls();
    assert!(
        !calls.iter().any(|c| c.starts_with("cancel_run")),
        "{calls:?}"
    );
}

#[test]
fn an_exit_that_beats_the_cancel_is_recorded_as_the_runs_own() {
    let mut f = local();
    let cancel;
    (f.cx, cancel) = f.cx.cancellable();
    let dir = f.box_run_dir(1);
    let waited = dir.clone();
    f.driver.before_cancel(move || {
        let pid = leader(&waited);
        assert!(
            eventually(|| waited.join("exit").exists() && live_in_group(pid) == 0),
            "the run exited before the cancel reached it"
        );
    });
    let v = thread::scope(|s| {
        s.spawn(|| {
            leader(&dir);
            cancel.cancel();
        });
        run(&f.cx, &req(&["echo started; sleep 1; exit 4"])).unwrap()
    });
    assert_eq!((v.exit, v.interrupted), (Some(4), false));
    assert!(f.driver.calls().contains(&"cancel_run box-1 1".to_owned()));
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert_eq!((row.exit, row.interrupted), (Some(4), false));
}

#[test]
fn a_follow_cut_five_times_with_no_progress_gives_up_and_cancels_the_run() {
    let f = local();
    f.driver.cut_every_follow(0);
    let dir = f.box_run_dir(1);
    let e = run(&f.cx, &req(&["echo started; sleep 60"])).unwrap_err();
    assert!(e.error.to_string().contains("cut 5 times"), "{e:?}");
    let follows = f
        .driver
        .calls()
        .iter()
        .filter(|c| c.starts_with("follow_run"))
        .count();
    assert_eq!(follows, 5);
    assert!(f.driver.calls().contains(&"cancel_run box-1 1".to_owned()));
    assert_eq!(live_in_group(leader(&dir)), 0, "the run's group is gone");
    let row = f.cx.store().unwrap().last_run(f.id).unwrap().unwrap();
    assert!(row.finished_at.is_some() && row.interrupted, "{row:?}");
}

#[test]
fn the_egress_proxy_a_run_started_outlives_the_runs_cancel() {
    let mut f = local();
    let cancel;
    (f.cx, cancel) = f.cx.cancellable();
    let dir = f.box_run_dir(1);
    let pid_file = f.box_dir.path().join("home/.moor/egress.pid");
    let leader_pid = thread::scope(|s| {
        let gone = s.spawn(|| {
            let pid = leader(&dir);
            cancel.cancel();
            pid
        });
        let v = run(&f.cx, &req(&["echo started; sleep 60"])).unwrap();
        assert!(v.interrupted);
        gone.join().unwrap()
    });
    assert_eq!(live_in_group(leader_pid), 0, "the run's group is gone");
    let proxy: u32 = std::fs::read_to_string(&pid_file)
        .expect("the run's prelude started the egress proxy")
        .trim()
        .parse()
        .unwrap();
    assert!(running(proxy), "the egress proxy survived the cancel");
}
