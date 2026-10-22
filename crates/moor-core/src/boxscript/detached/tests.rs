//! Tests for the detached-run scripts; each runs the generated text under `sh` with a scratch
//! `$HOME`. Tests that signal a process group belong inside a pid namespace.

use std::fs;
use std::io::Read;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::thread::sleep;
use std::time::Duration;

use rustix::process::{kill_process_group, Pid, Signal};

use super::{
    cancel_script, end_script, follow_script, launch_script, parse_end, End, Outcome, LOST, REFUSED,
};
use crate::boxscript::quote;

const RUN: i64 = 42;

/// Scratch `$HOME` and workdir, removed on drop after any run group left behind is killed.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("moor-detached-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("work")).unwrap();
        Self { root }
    }

    fn run_dir(&self) -> PathBuf {
        self.root.join(format!("home/.moor/runs/{RUN}"))
    }

    fn file(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.run_dir().join(name)).ok()
    }

    fn sh(&self, script: &str) -> Command {
        let mut c = Command::new("sh");
        c.arg("-c")
            .arg(script)
            .env("HOME", self.root.join("home"))
            .current_dir(self.root.join("work"));
        c
    }

    fn run(&self, script: &str) -> Output {
        self.sh(script).output().unwrap()
    }

    fn pid(&self) -> Option<i32> {
        String::from_utf8(self.file("pid")?)
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn launch(&self, command: &str) -> i32 {
        let out = self.run(&launch_script(RUN, command));
        assert_eq!(out.status.code(), Some(0), "launch: {out:?}");
        let printed: i32 = String::from_utf8(out.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(Some(printed), self.pid(), "launch prints the pid it wrote");
        printed
    }

    fn wait_exit(&self) {
        assert!(
            eventually(|| self.file("exit").is_some()),
            "the run never wrote exit"
        );
    }

    fn end(&self) -> End {
        let out = self.run(&end_script(RUN));
        assert_eq!(out.status.code(), Some(0), "end: {out:?}");
        parse_end(&String::from_utf8(out.stdout).unwrap()).expect("end prints a report")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(pid) = self.pid().filter(|p| *p > 1).and_then(Pid::from_raw) {
            let _ = kill_process_group(pid, Signal::KILL);
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Polls `ok` for up to 20 s.
fn eventually(mut ok: impl FnMut() -> bool) -> bool {
    for _ in 0..400 {
        if ok() {
            return true;
        }
        sleep(Duration::from_millis(50));
    }
    false
}

/// Processes in group `pgid` that have not exited; zombies nobody reaped yet do not count.
fn live_in_group(pgid: i32) -> usize {
    let Ok(dir) = fs::read_dir("/proc") else {
        return 0;
    };
    dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
                return false;
            };
            let Some((_, rest)) = stat.rsplit_once(')') else {
                return false;
            };
            let mut fields = rest.split_whitespace();
            let state = fields.next();
            let pgrp = fields.nth(1).and_then(|g| g.parse::<i32>().ok());
            state != Some("Z") && pgrp == Some(pgid)
        })
        .count()
}

/// The boot id and the start time of `pid`, as `start` holds them.
fn ident(pid: u32) -> (String, String) {
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    let proc_stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let (_, rest) = proc_stat.rsplit_once(')').unwrap();
    let start = rest.split_whitespace().nth(19).unwrap().to_owned();
    (boot.trim().to_owned(), start)
}

/// Waits up to `secs` for `child`, then kills it; `None` when it had to be killed.
fn wait_bounded(child: &mut Child, secs: u64) -> Option<ExitStatus> {
    for _ in 0..secs * 20 {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// A pid that belonged to a process that has exited and been reaped.
fn dead_pid() -> u32 {
    let mut child = Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn command_outlives_a_sigkill_of_the_launching_group() {
    let sb = Scratch::new("survive");
    let command = "printf one; sleep 1; printf two; sleep 1; printf three; exit 7";
    let launcher_script = format!("sh -c {}; sleep 30", quote(&launch_script(RUN, command)));
    let mut launcher = sb
        .sh(&launcher_script)
        .process_group(0)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    assert!(eventually(|| sb.pid().is_some()), "launch wrote no pid");
    let launcher_group = Pid::from_raw(i32::try_from(launcher.id()).unwrap()).unwrap();
    kill_process_group(launcher_group, Signal::KILL).unwrap();
    let status = launcher.wait().unwrap();
    assert_eq!(
        status.signal(),
        Some(9),
        "the launching group died by SIGKILL"
    );
    assert!(
        sb.file("exit").is_none(),
        "the command was still running at the kill"
    );
    assert_ne!(sb.pid(), Some(i32::try_from(launcher.id()).unwrap()));

    sb.wait_exit();
    assert_eq!(sb.file("log").unwrap(), b"onetwothree");
    assert_eq!(sb.file("exit").unwrap(), b"7\n");
}

#[test]
fn launch_refuses_a_run_dir_that_already_holds_a_run() {
    let sb = Scratch::new("twice");
    sb.launch("printf first");
    sb.wait_exit();
    let out = sb.run(&launch_script(RUN, "printf second"));
    assert_eq!(out.status.code(), Some(REFUSED), "{out:?}");
    sleep(Duration::from_millis(300));
    assert_eq!(sb.file("log").unwrap(), b"first");
}

#[test]
fn follow_from_an_offset_prints_exactly_the_rest_and_returns_after_exit() {
    let sb = Scratch::new("follow");
    let full: &[u8] = b"ab\0\xffcd\n\x01ef";
    sb.launch(r"printf 'ab\000\377'; sleep 1; printf 'cd\n\001'; sleep 1; printf ef");
    let mut follower = sb
        .sh(&follow_script(RUN, 3))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = follower.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut got = Vec::new();
        stdout.read_to_end(&mut got).unwrap();
        tx.send(got).unwrap();
    });
    let got = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("follow returns after exit");
    assert_eq!(got, full.get(3..).unwrap());
    assert_eq!(follower.wait().unwrap().code(), Some(0));
    assert_eq!(sb.file("log").unwrap(), full);

    let again = sb.run(&follow_script(RUN, 0));
    assert_eq!(
        again.stdout, full,
        "a follow after exit prints the whole log at once"
    );
}

#[test]
fn a_dead_pid_with_no_exit_reads_as_lost() {
    let sb = Scratch::new("lost");
    fs::create_dir_all(sb.run_dir()).unwrap();
    fs::write(sb.run_dir().join("pid"), format!("{}\n", dead_pid())).unwrap();
    fs::write(sb.run_dir().join("log"), b"partial").unwrap();

    assert_eq!(sb.end().outcome, Outcome::Lost);
    let out = sb.run(&follow_script(RUN, 0));
    assert_eq!(out.status.code(), Some(LOST), "{out:?}");
    assert_eq!(out.stdout, b"partial");
}

#[test]
fn end_reports_running_then_exit_and_the_moved_marker() {
    let sb = Scratch::new("end");
    sb.launch("sleep 1; mkdir -p .moor && : > .moor/record-moved; exit 7");
    assert_eq!(
        sb.end(),
        End {
            outcome: Outcome::Running,
            cancelled: false,
            moved: false
        }
    );
    sb.wait_exit();
    assert_eq!(
        sb.end(),
        End {
            outcome: Outcome::Exited(7),
            cancelled: false,
            moved: true
        }
    );
}

#[test]
fn cancel_stops_every_process_in_the_group() {
    let sb = Scratch::new("cancel");
    let pid = sb.launch("sleep 30 & sleep 30 & wait");
    assert!(
        eventually(|| live_in_group(pid) >= 3),
        "the run's group never filled"
    );

    let out = sb.run(&cancel_script(RUN, 2));
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(
        eventually(|| live_in_group(pid) == 0),
        "the run's group outlived cancel"
    );
    assert!(sb.file("cancelled").is_some());
    assert!(sb.end().cancelled);
}

#[test]
fn cancel_refuses_a_pid_it_must_not_signal() {
    for pid in [
        None,
        Some(""),
        Some("abc\n"),
        Some("0\n"),
        Some("1\n"),
        Some("-5\n"),
    ] {
        let sb = Scratch::new("refuse");
        fs::create_dir_all(sb.run_dir()).unwrap();
        if let Some(text) = pid {
            fs::write(sb.run_dir().join("pid"), text).unwrap();
        }
        let out = sb.run(&cancel_script(RUN, 1));
        assert_eq!(out.status.code(), Some(REFUSED), "pid {pid:?}: {out:?}");
        assert!(sb.file("cancelled").is_none(), "pid {pid:?}");
    }
}

#[test]
fn a_pid_reused_by_an_unrelated_group_leader_is_not_the_run() {
    let mut stranger = Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let (boot, start) = ident(stranger.id());
    for (label, recorded) in [
        ("another start time", format!("{boot} 1{start}\n")),
        (
            "another boot",
            format!("00000000-0000-0000-0000-000000000000 {start}\n"),
        ),
    ] {
        let sb = Scratch::new("reused");
        fs::create_dir_all(sb.run_dir()).unwrap();
        fs::write(sb.run_dir().join("start"), recorded).unwrap();
        fs::write(sb.run_dir().join("pid"), format!("{}\n", stranger.id())).unwrap();
        fs::write(sb.run_dir().join("log"), b"partial").unwrap();

        assert_eq!(sb.end().outcome, Outcome::Lost, "{label}");
        let mut follow = sb
            .sh(&follow_script(RUN, 0))
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        assert_eq!(
            wait_bounded(&mut follow, 5).and_then(|s| s.code()),
            Some(LOST),
            "{label}: follow ends as lost"
        );
        let out = sb.run(&cancel_script(RUN, 1));
        assert_eq!(out.status.code(), Some(REFUSED), "{label}: {out:?}");
        assert!(sb.file("cancelled").is_none(), "{label}");
        sleep(Duration::from_millis(200));
        assert!(
            stranger.try_wait().unwrap().is_none(),
            "{label}: the unrelated process survives cancel"
        );
        fs::remove_file(sb.run_dir().join("pid")).unwrap();
    }
    stranger.kill().unwrap();
    stranger.wait().unwrap();
}

#[test]
fn launch_records_the_leader_it_started() {
    let sb = Scratch::new("ident");
    let pid = sb.launch("sleep 30");
    let (boot, start) = ident(u32::try_from(pid).unwrap());
    assert_eq!(
        sb.file("start").unwrap(),
        format!("{boot} {start}\n").into_bytes()
    );
    assert_eq!(sb.end().outcome, Outcome::Running);
}

#[test]
fn a_reader_never_sees_a_partial_exit_file() {
    // A `mv` shim pauses the run at the step that publishes its exit code and has `end` read
    // the run there, so the check does not depend on timing.
    let sb = Scratch::new("atomic");
    let bin = sb.root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let real_mv = String::from_utf8(sb.run("command -v mv").stdout).unwrap();
    let shim = format!(
        "#!/bin/sh\nfor __last; do :; done\ncase \"$__last\" in */exit) sh -c \"$MOOR_TEST_END\" >> \"$HOME/seen\" ;; esac\nexec {} \"$@\"\n",
        real_mv.trim()
    );
    fs::write(bin.join("mv"), shim).unwrap();
    fs::set_permissions(
        bin.join("mv"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = sb
        .sh(&launch_script(RUN, "printf out; exit 7"))
        .env("PATH", &path)
        .env("MOOR_TEST_END", end_script(RUN))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "launch: {out:?}");
    sb.wait_exit();
    let seen = fs::read_to_string(sb.root.join("home/seen")).unwrap_or_default();
    assert_eq!(
        seen.lines().collect::<Vec<_>>(),
        ["exit=running cancelled=0 moved=0"],
        "just before the exit code lands, end reads the run as running"
    );
    assert_eq!(sb.file("exit").unwrap(), b"7\n");
    assert_eq!(sb.end().outcome, Outcome::Exited(7));
}

#[test]
fn parse_end_reads_each_outcome() {
    let end = |outcome, cancelled, moved| {
        Some(End {
            outcome,
            cancelled,
            moved,
        })
    };
    assert_eq!(
        parse_end("exit=0 cancelled=0 moved=0\n"),
        end(Outcome::Exited(0), false, false)
    );
    assert_eq!(
        parse_end("exit=255 cancelled=1 moved=1"),
        end(Outcome::Exited(255), true, true)
    );
    assert_eq!(
        parse_end("exit=running cancelled=0 moved=0"),
        end(Outcome::Running, false, false)
    );
    assert_eq!(
        parse_end("exit=lost cancelled=1 moved=0"),
        end(Outcome::Lost, true, false)
    );
}

#[test]
fn parse_end_rejects_anything_else() {
    for text in [
        "",
        "exit=256 cancelled=0 moved=0",
        "exit=-1 cancelled=0 moved=0",
        "exit=0 cancelled=2 moved=0",
        "exit=0 moved=0 cancelled=0",
        "exit=0 cancelled=0 moved=0 extra",
        "exit=0 cancelled=0",
    ] {
        assert_eq!(parse_end(text), None, "{text:?}");
    }
}
