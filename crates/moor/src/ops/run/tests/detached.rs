//! A run detached in the Box, on the fake that plays the Box's run scripts with the local sh.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_core::boxscript::record::{HEAD_FILE, HEAD_MOVED};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::ExecOutcome;
use moor_core::{BoxPath, RunId, RunReq};

use super::{ready_with, req, Fx};
use crate::ops::run::run;
use crate::testing::clock::FakeClock;
use crate::testing::driver::{eventually, live_in_group, FakeDriver};
use crate::testing::proxy_host::FakeProxyHost;
use moor_testkit::FakeEgress;

/// The Box workdir every fixture Box gets.
const WORKDIR: &str = "/work/cbx_1/repo";

/// A Ready Box whose runs play out under `<tmp>/home` and `<tmp>/work` on this host, with a
/// fake egress proxy in its `$HOME/bin`.
struct Local {
    f: Fx,
    box_dir: tempfile::TempDir,
    _egress: FakeEgress,
}

fn local() -> Local {
    local_with(FakeDriver::new())
}

fn local_with(driver: FakeDriver) -> Local {
    let box_dir = tempfile::tempdir().unwrap();
    let (home, work) = (box_dir.path().join("home"), box_dir.path().join("work"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let bin = home.join("bin");
    moor_testkit::write_host_guards(&bin).unwrap();
    let egress = FakeEgress::start(&home, &bin).unwrap();
    let mut f = ready_with(driver.local_runs(&home, &work));
    let host = FakeProxyHost::new().egress_port(egress.port());
    f.cx = f.cx.with_proxy_host(Arc::new(host));
    Local {
        f,
        box_dir,
        _egress: egress,
    }
}

impl Local {
    fn box_run_dir(&self, id: i64) -> PathBuf {
        self.box_dir.path().join(format!("home/.moor/runs/{id}"))
    }

    fn host_log(&self, id: RunId) -> Vec<u8> {
        std::fs::read(self.f.dir.path().join(format!("runs/{}.log", id.get()))).unwrap()
    }

    fn box_log(&self, id: RunId) -> Vec<u8> {
        std::fs::read(self.box_run_dir(id.get()).join("log")).unwrap()
    }

    fn box_pid(&self, id: i64) -> u32 {
        leader(&self.box_run_dir(id))
    }
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
    let l = local();
    let v = run(&l.f.cx, &req(&[NOISY])).unwrap();
    let id = v.run.unwrap();
    assert_eq!((v.exit, v.interrupted), (Some(7), false));
    let host = l.host_log(id);
    assert!(
        host.starts_with(b"bin\0\xff\x80 end\n"),
        "{:?}",
        host.get(..12)
    );
    assert_eq!(host, l.box_log(id));
    let row = l.f.cx.store().unwrap().run(id).unwrap().unwrap();
    assert_eq!((row.exit, row.interrupted), (Some(7), false));
    assert_eq!(row.pid, l.box_pid(id.get()), "runs.pid is the Box pid");
    let calls = l.f.driver.calls();
    assert!(calls.iter().all(|c| !c.starts_with("exec")), "{calls:?}");
}

#[test]
fn a_cut_follow_resumes_from_the_offset_it_reached() {
    let l = local();
    l.f.driver.cut_next_follow(1000);
    let v = run(&l.f.cx, &req(&[NOISY])).unwrap();
    let id = v.run.unwrap();
    assert_eq!(v.exit, Some(7));
    assert_eq!(l.host_log(id), l.box_log(id));
    let follows: Vec<String> =
        l.f.driver
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("follow_run"))
            .collect();
    let want = format!("follow_run box-1 {}", id.get());
    assert_eq!(follows, [format!("{want} 0"), format!("{want} 1000")]);
}

#[test]
fn a_box_process_killed_without_an_exit_is_recorded_interrupted() {
    let l = local();
    let leader = r#"echo before; kill -s KILL "$(cat "$HOME/.moor/runs/1/pid")"; sleep 5"#;
    let v = run(&l.f.cx, &req(&[leader])).unwrap();
    let id = v.run.unwrap();
    assert_eq!((v.exit, v.interrupted), (None, true));
    let row = l.f.cx.store().unwrap().run(id).unwrap().unwrap();
    assert_eq!((row.exit, row.interrupted), (None, true));
    assert_eq!(l.host_log(id), l.box_log(id));
    assert!(l.host_log(id).starts_with(b"before\n"));
}

/// A git checkout in `work` with one commit; returns its sha.
fn checkout(work: &Path) -> String {
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(work)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    git(&["init", "-q"]);
    std::fs::write(work.join("tracked"), "one\n").unwrap();
    git(&["add", "tracked"]);
    git(&["commit", "-qm", "one"]);
    git(&["rev-parse", "HEAD"]).trim().to_owned()
}

#[test]
fn a_record_run_that_moves_the_checkout_is_recorded_head_moved() {
    let l = local_with(FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 0 })]));
    let sha = checkout(&l.box_dir.path().join("work"));
    let head = BoxPath::under(WORKDIR, HEAD_FILE).unwrap();
    l.f.driver
        .set_peek_text("box-1", &head, &format!("head={sha} dirty=0\n"));
    let r = RunReq {
        record: Some("ci".into()),
        ..req(&["echo two >> tracked"])
    };
    let v = run(&l.f.cx, &r).unwrap();
    assert_ne!(v.exit, Some(0));
    let events = l.f.cx.store().unwrap().events(l.f.id, 1000).unwrap();
    let ran = events.iter().rfind(|e| e.kind == "ran").unwrap();
    assert_eq!(
        ran.payload.get("error"),
        Some(&HEAD_MOVED.into()),
        "{:?}",
        ran.payload
    );
}

#[test]
fn a_passed_deadline_cancels_the_run_in_the_box_and_is_recorded() {
    let mut l = local();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    l.f.cx = l.f.cx.with_clock(clock.clone());
    let dir = l.box_run_dir(1);
    let pid = thread::scope(|s| {
        let late = s.spawn(|| {
            let pid = leader(&dir);
            clock.sleep(Duration::from_hours(25));
            pid
        });
        let e = run(&l.f.cx, &req(&["echo started; sleep 60"])).unwrap_err();
        assert!(e.error.to_string().contains("deadline"), "{e:?}");
        late.join().unwrap()
    });
    assert_eq!(live_in_group(pid), 0, "the run's group is gone");
    let row = l.f.cx.store().unwrap().last_run(l.f.id).unwrap().unwrap();
    assert!(row.interrupted && row.finished_at.is_some(), "{row:?}");
    assert!(l
        .f
        .driver
        .calls()
        .contains(&"cancel_run box-1 1".to_owned()));
}

#[test]
fn a_client_hangup_cancels_the_run_in_the_box() {
    let mut l = local();
    let cancel;
    (l.f.cx, cancel) = l.f.cx.cancellable();
    let dir = l.box_run_dir(1);
    let pid = thread::scope(|s| {
        let gone = s.spawn(|| {
            let pid = leader(&dir);
            cancel.cancel();
            pid
        });
        let v = run(&l.f.cx, &req(&["echo started; sleep 60 & sleep 60"])).unwrap();
        assert_eq!((v.exit, v.interrupted), (None, true));
        gone.join().unwrap()
    });
    assert_eq!(live_in_group(pid), 0, "the run's group is gone");
    assert!(l
        .f
        .driver
        .calls()
        .contains(&"cancel_run box-1 1".to_owned()));
}
