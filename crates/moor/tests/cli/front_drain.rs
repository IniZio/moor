//! Fronts end with the controller's drain, and a front a crashed controller left behind is
//! stopped by the next one.
#![expect(
    clippy::disallowed_methods,
    reason = "tests wait on real child processes with deadlines"
)]

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rustix::process::{kill_process, Pid, Signal};

use super::{fresh_dir_outside_tmp, Dir};

/// Takes SIGTERM, takes a second to finish, writes `<name>.term`, exits 0.
const SLOW_TO_TERM: &str = r#"
import os, signal, sys, time
root, name = sys.argv[1], sys.argv[2]
def term(*_):
    time.sleep(1)
    open(f"{root}/{name}.term", "w").write("done")
    sys.exit(0)
signal.signal(signal.SIGTERM, term)
open(f"{root}/{name}.up", "a").write("x")
time.sleep(600)
"#;

/// Ignores SIGTERM.
const IGNORES_TERM: &str = r#"
import os, signal, sys, time
root, name = sys.argv[1], sys.argv[2]
signal.signal(signal.SIGTERM, signal.SIG_IGN)
open(f"{root}/{name}.up", "a").write("x")
while True:
    time.sleep(1)
"#;

/// Exits 1 as soon as it gets SIGTERM.
const FAILS_ON_TERM: &str = r#"
import os, signal, sys, time
root, name = sys.argv[1], sys.argv[2]
def term(*_):
    sys.exit(1)
signal.signal(signal.SIGTERM, term)
open(f"{root}/{name}.up", "a").write("x")
time.sleep(600)
"#;

/// Prints a 300 KiB line, then a line that is not UTF-8, then `after`.
const NOISY: &str = r#"
import os, sys, time
root, name = sys.argv[1], sys.argv[2]
out = sys.stdout.buffer
out.write(b"x" * 300_000 + b"\n")
out.write(b"bad \xff\xfe bytes\n")
out.write(b"after\n")
out.flush()
open(f"{root}/{name}.up", "a").write("x")
time.sleep(600)
"#;

/// Ignores SIGTERM and writes its first argument once it does; a leftover stand-in.
const LEFTOVER_IGNORING_TERM: &str = r#"
import signal, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
open(sys.argv[1], "w").write("up")
while True:
    time.sleep(1)
"#;

/// Takes two seconds to finish after SIGTERM, writes its second argument; its first argument is written once the handler is set.
const LEFTOVER_SLOW_TO_TERM: &str = r#"
import signal, sys, time
def term(*_):
    time.sleep(2)
    open(sys.argv[2], "w").write("done")
    sys.exit(0)
signal.signal(signal.SIGTERM, term)
open(sys.argv[1], "w").write("up")
time.sleep(600)
"#;

/// A state dir and config with some fronts that each run a script and may write `marks`.
struct Setup {
    root: Dir,
}

impl Setup {
    /// Fronts named by the pairs, each running its script with the root and its name; the drain
    /// gets `drain_secs`.
    fn new(drain_secs: u32, fronts: &[(&str, &str)]) -> Self {
        let root = fresh_dir_outside_tmp("frontdrain");
        let mut config = format!(
            "socket: {}\nstate_dir: {}\ndrain_timeout_secs: {drain_secs}\nfronts:\n",
            root.join("c.sock").display(),
            root.join("state").display(),
        );
        let marks = root.join("marks");
        for (name, script) in fronts {
            let command = serde_json::json!(["python3", "-c", script, &*marks, name]);
            let _ = writeln!(
                config,
                "  {name}:\n    command: {command}\n    state_dir: {}",
                marks.display()
            );
        }
        fs::write(root.join("c.yaml"), config).unwrap();
        Self { root }
    }

    /// Serves a controller on this setup; its log is appended to `log`.
    fn serve(&self) -> Child {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("log"))
            .unwrap();
        Command::new(env!("CARGO_BIN_EXE_moor"))
            .args(["controller", "serve", "--config"])
            .arg(self.root.join("c.yaml"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root.join("home"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.root.join("log")).unwrap_or_default()
    }

    /// How many times `name` started.
    fn starts(&self, name: &str) -> usize {
        fs::read_to_string(self.root.join("marks").join(format!("{name}.up")))
            .map_or(0, |s| s.len())
    }

    fn pid_file(&self, name: &str) -> PathBuf {
        self.root.join("state/fronts").join(format!("{name}.pid"))
    }

    /// The pid and start time recorded for `name`.
    fn recorded(&self, name: &str) -> Option<(u32, String)> {
        let text = fs::read_to_string(self.pid_file(name)).ok()?;
        let mut lines = text.lines();
        Some((lines.next()?.parse().ok()?, lines.next()?.to_owned()))
    }

    /// The pids whose command line carries this setup's root, apart from the controller.
    fn front_processes(&self) -> Vec<u32> {
        let marker = self.root.display().to_string();
        fs::read_dir("/proc")
            .unwrap()
            .filter_map(|e| {
                let pid: u32 = e.ok()?.file_name().to_str()?.parse().ok()?;
                let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
                let text = String::from_utf8_lossy(&cmdline);
                (text.contains(&marker) && !text.contains("controller")).then_some(pid)
            })
            .filter(|pid| alive(*pid))
            .collect()
    }
}

/// Whether `pid` exists and is not a zombie.
fn alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next().map(|s| s != "Z"))
            .unwrap_or(false)
    })
}

/// The start time `/proc` gives for `pid`.
fn start_time(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(19).map(str::to_owned)
}

fn pid_of(child: &Child) -> Pid {
    Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap()
}

/// Polls `check` until it returns something, for up to 20 s.
fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(got) = check() {
            return got;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Waits up to `within` for `child` to exit; its status once it did.
fn exit_within(child: &mut Child, within: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + within;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Serves until every front in `names` is up, with its pid file written.
fn serve_until_up(setup: &Setup, names: &[&str]) -> Child {
    let child = setup.serve();
    for name in names {
        wait_for("a front to start", || {
            (setup.starts(name) > 0).then_some(())?;
            setup.recorded(name).map(|_| ())
        });
    }
    child
}

#[test]
fn the_drain_terminates_each_front_and_the_controller_exits_after_they_are_reaped() {
    let setup = Setup::new(30, &[("slow", SLOW_TO_TERM)]);
    let mut ctl = serve_until_up(&setup, &["slow"]);
    assert!(!setup.front_processes().is_empty());
    kill_process(pid_of(&ctl), Signal::TERM).unwrap();
    let status = exit_within(&mut ctl, Duration::from_secs(20)).expect("the controller exited");
    assert!(status.success(), "{status}: {}", setup.log());
    assert!(
        setup.root.join("marks").join("slow.term").exists(),
        "the controller left before the front finished: {}",
        setup.log()
    );
    wait_for("every front process to exit", || {
        setup.front_processes().is_empty().then_some(())
    });
    assert!(!setup.pid_file("slow").exists());
}

#[test]
fn a_front_that_ignores_sigterm_is_killed_when_the_drain_budget_ends() {
    let setup = Setup::new(2, &[("stubborn", IGNORES_TERM)]);
    let mut ctl = serve_until_up(&setup, &["stubborn"]);
    let began = Instant::now();
    kill_process(pid_of(&ctl), Signal::TERM).unwrap();
    let status = exit_within(&mut ctl, Duration::from_secs(20)).expect("the controller exited");
    let took = began.elapsed();
    assert!(status.success(), "{status}: {}", setup.log());
    assert!(
        took >= Duration::from_secs(2),
        "{took:?} is under the budget"
    );
    assert!(took < Duration::from_secs(10), "{took:?}");
    wait_for("every front process to exit", || {
        setup.front_processes().is_empty().then_some(())
    });
}

#[test]
fn a_front_is_not_restarted_while_the_controller_drains() {
    let setup = Setup::new(3, &[("fails", FAILS_ON_TERM), ("stubborn", IGNORES_TERM)]);
    let mut ctl = serve_until_up(&setup, &["fails", "stubborn"]);
    kill_process(pid_of(&ctl), Signal::TERM).unwrap();
    assert!(exit_within(&mut ctl, Duration::from_secs(20)).is_some());
    assert_eq!(setup.starts("fails"), 1, "{}", setup.log());
    assert!(
        !setup.log().contains("front fails: exited"),
        "{}",
        setup.log()
    );
    wait_for("every front process to exit", || {
        setup.front_processes().is_empty().then_some(())
    });
}

#[test]
fn a_front_left_by_a_crashed_controller_is_stopped_on_the_next_start() {
    let setup = Setup::new(30, &[("slow", SLOW_TO_TERM)]);
    let mut first = serve_until_up(&setup, &["slow"]);
    kill_process(pid_of(&first), Signal::TERM).unwrap();
    assert!(exit_within(&mut first, Duration::from_secs(20)).is_some());

    let mut leftover = Command::new("sleep").arg("600").spawn().unwrap();
    let started = start_time(leftover.id()).unwrap();
    fs::write(
        setup.pid_file("slow"),
        format!("{}\n{started}\n", leftover.id()),
    )
    .unwrap();

    let mut next = setup.serve();
    wait_for("the next front", || {
        (setup.starts("slow") == 2).then_some(())
    });
    assert!(!alive(leftover.id()), "{}", setup.log());
    let (shim, recorded) = setup.recorded("slow").unwrap();
    assert_ne!(shim, leftover.id());
    assert_eq!(start_time(shim), Some(recorded));
    kill_process(pid_of(&next), Signal::TERM).unwrap();
    assert!(exit_within(&mut next, Duration::from_secs(20)).is_some());
    leftover.wait().unwrap();
}

#[test]
fn a_recycled_pid_with_another_start_time_is_left_alone() {
    let setup = Setup::new(30, &[("slow", SLOW_TO_TERM)]);
    let mut first = serve_until_up(&setup, &["slow"]);
    kill_process(pid_of(&first), Signal::TERM).unwrap();
    assert!(exit_within(&mut first, Duration::from_secs(20)).is_some());

    let mut bystander = Command::new("sleep").arg("600").spawn().unwrap();
    let real = start_time(bystander.id()).unwrap();
    let other = (real.parse::<u64>().unwrap() + 1).to_string();
    fs::write(
        setup.pid_file("slow"),
        format!("{}\n{other}\n", bystander.id()),
    )
    .unwrap();

    let mut next = setup.serve();
    wait_for("the next front", || {
        (setup.starts("slow") == 2).then_some(())
    });
    assert!(alive(bystander.id()), "{}", setup.log());
    assert_eq!(start_time(bystander.id()), Some(real));
    kill_process(pid_of(&next), Signal::TERM).unwrap();
    assert!(exit_within(&mut next, Duration::from_secs(20)).is_some());
    bystander.kill().unwrap();
    bystander.wait().unwrap();
}

/// Starts `python3 -c script args` as a leftover, with a pid file for front `name` naming it
/// by its real start time. The setup's state dir must exist: run and drain a controller first.
fn plant_leftover(setup: &Setup, name: &str, script: &str, args: &[&str]) -> Child {
    let ready = setup.root.join(format!("{name}.leftover-ready"));
    let leftover = Command::new("python3")
        .args(["-c", script])
        .arg(&ready)
        .args(args)
        .spawn()
        .unwrap();
    wait_for("the leftover to be ready", || ready.exists().then_some(()));
    let started = wait_for("the leftover's start time", || start_time(leftover.id()));
    fs::write(
        setup.pid_file(name),
        format!("{}\n{started}\n", leftover.id()),
    )
    .unwrap();
    leftover
}

/// Starts and drains a controller once, so the state dir exists and no front runs.
fn prepare_state(setup: &Setup, names: &[&str]) {
    let mut first = serve_until_up(setup, names);
    kill_process(pid_of(&first), Signal::TERM).unwrap();
    assert!(exit_within(&mut first, Duration::from_secs(20)).is_some());
    for name in names {
        let _ = fs::remove_file(setup.root.join("marks").join(format!("{name}.up")));
    }
}

#[test]
fn a_leftover_that_ignores_sigterm_is_killed_after_the_grace() {
    let setup = Setup::new(30, &[("slow", SLOW_TO_TERM)]);
    prepare_state(&setup, &["slow"]);
    let mut leftover = plant_leftover(&setup, "slow", LEFTOVER_IGNORING_TERM, &[]);
    let began = Instant::now();
    let mut next = setup.serve();
    wait_for("the leftover to go", || {
        (!alive(leftover.id())).then_some(())
    });
    let took = began.elapsed();
    assert!(
        took >= Duration::from_secs(4),
        "{took:?} is under the grace"
    );
    wait_for("the next front", || {
        (setup.starts("slow") == 1).then_some(())
    });
    kill_process(pid_of(&next), Signal::TERM).unwrap();
    assert!(exit_within(&mut next, Duration::from_secs(20)).is_some());
    leftover.wait().unwrap();
}

#[test]
fn a_leftover_gets_the_grace_to_finish_after_sigterm() {
    let setup = Setup::new(30, &[("slow", SLOW_TO_TERM)]);
    prepare_state(&setup, &["slow"]);
    let done = setup.root.join("leftover.done");
    let mut leftover = plant_leftover(
        &setup,
        "slow",
        LEFTOVER_SLOW_TO_TERM,
        &[done.to_str().unwrap()],
    );
    let mut next = setup.serve();
    wait_for("the next front", || {
        (setup.starts("slow") == 1).then_some(())
    });
    assert!(done.exists(), "the leftover was killed inside the grace");
    kill_process(pid_of(&next), Signal::TERM).unwrap();
    assert!(exit_within(&mut next, Duration::from_secs(20)).is_some());
    leftover.wait().unwrap();
}

#[test]
fn a_drain_that_begins_while_a_leftover_is_stopped_starts_no_front() {
    let setup = Setup::new(30, &[("slow", SLOW_TO_TERM)]);
    prepare_state(&setup, &["slow"]);
    let mut leftover = plant_leftover(&setup, "slow", LEFTOVER_IGNORING_TERM, &[]);
    let started_before = setup.log().matches("started pid").count();
    let mut next = setup.serve();
    wait_for("the stop to begin", || {
        setup.log().contains("stopping pid").then_some(())
    });
    kill_process(pid_of(&next), Signal::TERM).unwrap();
    assert!(exit_within(&mut next, Duration::from_secs(20)).is_some());
    assert_eq!(
        setup.log().matches("started pid").count(),
        started_before,
        "a front started during the drain: {}",
        setup.log()
    );
    leftover.kill().unwrap();
    leftover.wait().unwrap();
}

#[test]
fn a_long_line_is_cut_and_a_line_that_is_not_utf8_does_not_end_the_log() {
    let setup = Setup::new(30, &[("noisy", NOISY)]);
    let mut ctl = serve_until_up(&setup, &["noisy"]);
    wait_for("the last line", || {
        setup.log().contains("front noisy: after").then_some(())
    });
    let log = setup.log();
    assert!(log.len() < 200_000, "{} bytes logged", log.len());
    assert!(log.contains("[line cut at 64 KiB]"), "cut marker missing");
    assert!(log.contains("front noisy: bad \u{fffd}"), "{log}");
    assert_eq!(setup.starts("noisy"), 1, "{log}");
    kill_process(pid_of(&ctl), Signal::TERM).unwrap();
    assert!(exit_within(&mut ctl, Duration::from_secs(20)).is_some());
}
