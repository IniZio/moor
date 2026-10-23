use std::process::{Child, Command};

use rustix::process::{Pid, Signal};

use super::{running, Target};

fn sleeper() -> (Child, Pid, String) {
    let child = Command::new("sleep").arg("600").spawn().expect("spawn");
    let raw = i32::try_from(child.id()).expect("pid");
    let started = std::fs::read_to_string(format!("/proc/{raw}/stat"))
        .expect("stat")
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(19).map(str::to_owned))
        .expect("start time");
    (child, Pid::from_raw(raw).expect("pid"), started)
}

#[test]
fn a_target_with_the_recorded_start_time_is_signalled() {
    let (mut child, pid, started) = sleeper();
    let target = Target::open(pid, &started).expect("opens");
    assert!(target.signal(Signal::KILL));
    child.wait().expect("wait");
}

#[test]
fn a_pid_with_another_start_time_does_not_open() {
    let (mut child, pid, started) = sleeper();
    let other = (started.parse::<u64>().expect("ticks") + 1).to_string();
    assert!(Target::open(pid, &other).is_none());
    assert!(running(pid.as_raw_nonzero().get(), &started));
    child.kill().expect("kill");
    child.wait().expect("wait");
}

#[test]
fn a_reaped_process_does_not_open_and_a_held_one_cannot_be_signalled() {
    let (mut child, pid, started) = sleeper();
    let target = Target::open(pid, &started).expect("opens");
    child.kill().expect("kill");
    child.wait().expect("wait");
    assert!(!target.signal(Signal::TERM));
    assert!(Target::open(pid, &started).is_none());
}
