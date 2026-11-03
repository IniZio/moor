use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use moor_testkit::ChildGuard;
use rustix::process::{kill_process, Pid, Signal};

/// Starts `command` with the runner's flags, waits until it listens and returns it with the pid of
/// the runner itself, which is not the child's when `command` is a wrapper.
fn start(mut command: Command, dir: &std::path::Path) -> (ChildGuard, Pid) {
    let mut child = ChildGuard::new(
        command
            .args(["--listen", "127.0.0.1:0", "--socket"])
            .arg(dir.join("runner.sock"))
            .arg("--state-dir")
            .arg(dir.join("state"))
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stderr = child.stderr.take().unwrap();
    let mut line = String::new();
    BufReader::new(stderr).read_line(&mut line).unwrap();
    assert!(line.contains("listening"), "unexpected first line: {line}");
    let pid = i32::try_from(child.id()).unwrap();
    (child, Pid::from_raw(pid).unwrap())
}

/// Sends SIGTERM to `pid` and asserts the child exits with status 0 within a few seconds.
fn assert_exits_cleanly(mut child: ChildGuard, pid: Pid) {
    kill_process(pid, Signal::TERM).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "still running 5 s after SIGTERM");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(0), "exit status was {status}");
}

#[test]
fn sigterm_ends_the_runner_with_status_zero() {
    let dir = moor_testkit::short_tempdir("msig").unwrap();
    let runner = Command::new(env!("CARGO_BIN_EXE_moor-machine-runner"));
    let (child, pid) = start(runner, dir.path());
    assert_exits_cleanly(child, pid);
}

/// The runner as pid 1 of its own namespace, as in the pod: pid 1 gets no default SIGTERM action.
#[test]
fn sigterm_ends_the_runner_as_pid_1() {
    let probe = Command::new("unshare")
        .args(["-Urpf", "true"])
        .status()
        .is_ok_and(|s| s.success());
    if !probe {
        eprintln!("skipped: no unprivileged user and pid namespaces here");
        return;
    }
    let dir = moor_testkit::short_tempdir("msig").unwrap();
    let mut unshare = Command::new("unshare");
    unshare
        .args(["-Urpf", "--kill-child", "--mount-proc"])
        .arg(env!("CARGO_BIN_EXE_moor-machine-runner"));
    let (child, unshare_pid) = start(unshare, dir.path());
    let children = Command::new("pgrep")
        .arg("-P")
        .arg(unshare_pid.as_raw_nonzero().to_string())
        .output()
        .unwrap();
    let children = String::from_utf8(children.stdout).unwrap();
    let runner = children.split_whitespace().next().unwrap().parse().unwrap();
    assert_exits_cleanly(child, Pid::from_raw(runner).unwrap());
}
