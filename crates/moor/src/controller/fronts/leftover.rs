//! Stops a front that a controller which crashed left running.

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{kill_process, Pid, Signal};

use super::{log, Front};
use crate::controller::Sink;

/// How long a leftover front gets to exit after each signal.
const GRACE: Duration = Duration::from_secs(5);

/// How often a leftover is checked for having exited.
const POLL: Duration = Duration::from_millis(50);

/// Stops the front the pid file of `front` names, if that process is still the one recorded
/// there: a pid whose start time differs belongs to another process now and is left alone.
/// Sends SIGTERM, then SIGKILL after the grace. Removes the pid file.
pub(super) fn stop(front: &Front, sink: &Sink) {
    let ns = &front.ns;
    let path = front.pid_file();
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let _ = fs::remove_file(&path);
    let mut lines = text.lines();
    let (Some(pid), Some(started)) = (
        lines
            .next()
            .and_then(|l| l.parse::<i32>().ok())
            .and_then(Pid::from_raw),
        lines.next(),
    ) else {
        log(
            sink,
            &format!("front {ns}: ignoring an unreadable pid file"),
        );
        return;
    };
    let raw = pid.as_raw_nonzero().get();
    if !running(raw, started) {
        log(
            sink,
            &format!(
                "front {ns}: pid {raw} is not the front a crashed controller left; leaving it"
            ),
        );
        return;
    }
    log(
        sink,
        &format!("front {ns}: stopping pid {raw}, left by a controller that did not exit"),
    );
    for signal in [Signal::TERM, Signal::KILL] {
        let _ = kill_process(pid, signal);
        if gone_within(raw, started) {
            return;
        }
    }
    log(sink, &format!("front {ns}: pid {raw} did not exit"));
}

/// Whether the process is gone within the grace.
#[expect(
    clippy::disallowed_methods,
    reason = "times a grace period for a process, not domain time"
)]
fn gone_within(pid: i32, started: &str) -> bool {
    let deadline = Instant::now() + GRACE;
    while running(pid, started) {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL);
    }
    true
}

/// Whether `pid` is a live process that started at `started`: not a zombie, and not a later
/// process that got the same pid.
fn running(pid: i32, started: &str) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return false;
    };
    let mut fields = rest.split_whitespace();
    fields.next().is_some_and(|state| state != "Z") && fields.nth(18) == Some(started)
}
