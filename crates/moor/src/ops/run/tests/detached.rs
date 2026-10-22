//! A run detached in the Box, on the fake that plays the Box's run scripts with the local sh.

use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_core::seam::clock::{Clock, Ts};

use super::{ready_with, req, Fx};
use crate::ops::run::run;
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
