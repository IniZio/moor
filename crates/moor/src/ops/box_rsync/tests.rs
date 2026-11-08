//! `box-rsync` against the fake Driver, which runs the Box's `rsync --server` on the host.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::proc::Pipes;
use moor_core::view::ExecView;
use moor_core::{BoxRsyncReq, ErrorKind};

use moor_core::seam::clock::{Clock, Ts};

use super::{run, DEADLINE};
use crate::error::CommandError;
use crate::ops::run_fixture::{ready_with, Fx as RunFx};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;

/// rsync's remote shell for these tests: it saves the command rsync gives it as `argv`, then
/// carries rsync's stdio over the FIFOs `up` and `down`, which the test feeds to `run`.
const RSH: &str = r#"#!/bin/sh
shift
printf '%s\0' "$@" >"$RSH_DIR/argv.tmp" && mv "$RSH_DIR/argv.tmp" "$RSH_DIR/argv"
exec 3<&0
cat <&3 >"$RSH_DIR/up" &
exec cat <"$RSH_DIR/down"
"#;

struct Fx {
    run: RunFx,
    /// Holds the Box's work tree and the test's own files.
    work: tempfile::TempDir,
    clock: Arc<FakeClock>,
}

/// Box `t`, ready on a fake machine whose work tree sits under `work`.
fn fx() -> Fx {
    let work = tempfile::tempdir().unwrap();
    let root = work.path().display().to_string();
    let mut run = ready_with(FakeDriver::new().work_root(&root));
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    run.cx = run.cx.with_clock(clock.clone());
    let f = Fx { run, work, clock };
    std::fs::create_dir_all(f.tree()).unwrap();
    f
}

impl Fx {
    /// The Box's work tree, as the fake driver lays it out.
    fn tree(&self) -> PathBuf {
        self.work.path().join("cbx_1/repo")
    }

    fn req(&self, argv: &[&str]) -> BoxRsyncReq {
        BoxRsyncReq {
            id: self.run.id,
            argv: argv.iter().map(|w| (*w).to_owned()).collect(),
        }
    }

    fn piped_execs(&self) -> usize {
        let calls = self.run.driver.calls();
        calls.iter().filter(|c| c.starts_with("exec_piped")).count()
    }

    /// Runs `rsync` with `args` against the Box through [`run`], as the runner's remote shell
    /// would; the controller's answer and how the rsync client exited.
    fn rsync(&self, args: &[&str]) -> (Result<ExecView, CommandError>, bool) {
        let rsh = self.work.path().join("rsh");
        let _ = std::fs::create_dir(&rsh);
        moor_testkit::write_executable(&rsh.join("sh"), RSH).unwrap();
        let made = Command::new("mkfifo")
            .arg(rsh.join("up"))
            .arg(rsh.join("down"))
            .status()
            .unwrap();
        assert!(made.success(), "mkfifo");
        let mut client = moor_testkit::ChildGuard::new(
            Command::new("rsync")
                .args(args)
                .arg("-e")
                .arg(rsh.join("sh"))
                .env("RSH_DIR", &rsh)
                .spawn()
                .unwrap(),
        );
        let sent = wait_for(&rsh.join("argv"));
        let words: Vec<&str> = sent.split('\0').filter(|w| !w.is_empty()).collect();
        let down_path = rsh.join("down");
        let down = std::thread::spawn(move || File::options().write(true).open(down_path).unwrap());
        let up = File::open(rsh.join("up")).unwrap();
        let mut down = down.join().unwrap();
        let mut stderr = Vec::new();
        let got = run(
            &self.run.cx,
            &self.req(&words),
            Pipes {
                stdin: Box::new(up),
                stdout: &mut down,
                stderr: &mut stderr,
                stop: &|| false,
            },
            DEADLINE,
        );
        drop(down);
        let ok = client.wait().unwrap().success();
        (got, ok)
    }
}

/// The contents of `path` once it exists.
fn wait_for(path: &Path) -> String {
    for _ in 0..200 {
        if path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    std::fs::read_to_string(path).expect("rsync started its remote shell")
}

#[test]
fn a_mirror_copies_the_work_tree_and_deletes_what_left_it() {
    let f = fx();
    std::fs::create_dir_all(f.tree().join("src")).unwrap();
    std::fs::write(f.tree().join("src/a.rs"), "fn a() {}").unwrap();
    let mirror = f.work.path().join("mirror");
    std::fs::create_dir_all(&mirror).unwrap();
    std::fs::write(mirror.join("stale"), "old").unwrap();
    let to = format!("{}/", mirror.display());
    let (got, ok) = f.rsync(&["-a", "--delete", "box:./", &to]);
    assert_eq!(
        got.unwrap(),
        ExecView {
            exit: Some(0),
            timed_out: false,
        }
    );
    assert!(ok, "the rsync client succeeded");
    let copied = std::fs::read_to_string(mirror.join("src/a.rs")).unwrap();
    assert_eq!(copied, "fn a() {}");
    assert!(!mirror.join("stale").exists());
}

#[test]
fn a_put_lands_beneath_the_work_tree_making_its_directories() {
    let f = fx();
    let staged = f.work.path().join("staged");
    std::fs::write(&staged, "built ok").unwrap();
    let staged = staged.display().to_string();
    let (got, ok) = f.rsync(&["--mkpath", "--chmod=F644", &staged, "box:out/my build.log"]);
    assert_eq!(
        got.unwrap(),
        ExecView {
            exit: Some(0),
            timed_out: false,
        }
    );
    assert!(ok, "the rsync client succeeded");
    let landed = f.tree().join("out/my build.log");
    assert_eq!(std::fs::read_to_string(landed).unwrap(), "built ok");
}

#[test]
fn an_argv_off_the_work_tree_or_with_other_options_is_refused_before_the_box_is_touched() {
    let f = fx();
    let tree = f.tree().display().to_string();
    let outside = format!("rsync --server -e.LsfxCIvu . {tree}/../x");
    let bad = [
        "rsync --server --sender -logDtpre.iLsfxCIvu . /etc/",
        "rsync --server -e.LsfxCIvu . ../x",
        &outside,
        "rsync --server -e.LsfxCIvu . a b",
        "rsync --server -se.LsfxCIvu . a",
        "rsync --server --secluded-args . a",
        "rsync --server --log-file=/tmp/x . a",
        "sh -c id",
        "rsync --sender . a",
    ];
    for argv in bad {
        let words: Vec<&str> = argv.split(' ').collect();
        let err = run(
            &f.run.cx,
            &f.req(&words),
            idle_pipes(&mut Vec::new()),
            DEADLINE,
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Refused, "{argv:?}");
    }
    assert_eq!(f.piped_execs(), 0);
}

#[test]
fn the_deadline_ends_a_stuck_transfer() {
    let f = fx();
    let argv: Vec<&str> = "rsync --server --sender -logDtpre.iLsfxCIvu . ./"
        .split(' ')
        .collect();
    let mut out = Vec::new();
    let (driver, clock) = (f.run.driver.clone(), f.clock.clone());
    std::thread::spawn(move || {
        while !driver.calls().iter().any(|c| c.starts_with("exec_piped")) {
            std::thread::sleep(Duration::from_millis(10));
        }
        clock.sleep(DEADLINE);
    });
    let err = run(&f.run.cx, &f.req(&argv), idle_pipes(&mut out), DEADLINE).unwrap_err();
    assert!(
        err.error.to_string().contains("10 minute deadline"),
        "{err:?}"
    );
}

/// Pipes whose stdin never sends a byte nor ends, as a stuck rsync client's would.
fn idle_pipes(out: &mut Vec<u8>) -> Pipes<'_> {
    struct Silent;
    impl Read for Silent {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(Duration::from_secs(3600));
            Ok(0)
        }
    }
    Pipes {
        stdin: Box::new(Silent),
        stdout: out,
        stderr: Box::leak(Box::new(Vec::new())),
        stop: &|| false,
    }
}
