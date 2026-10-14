//! Process-group signalling for `SystemProc`.

use std::io;

use moor_core::seam::proc::{check_group, ProcError, Signal as GroupSignal};
use rustix::process::{kill_process_group, Pid, Signal};

/// Converts a raw pid, rejecting 0 and values beyond `i32`.
pub(super) fn to_pid(pid: u32) -> Option<Pid> {
    i32::try_from(pid).ok().and_then(Pid::from_raw)
}

/// Sends `sig` to the process group led by `pid`; errors mean the group is gone.
pub(super) fn signal_group(pid: u32, sig: Signal) {
    if let Some(pid) = to_pid(pid) {
        kill_process_group(pid, sig).ok();
    }
}

/// Sends `sig` to group `pgid`, refusing the init and caller-wide groups and
/// reporting a missing group as an error.
pub(super) fn send_group(pgid: u32, sig: GroupSignal) -> Result<(), ProcError> {
    check_group(pgid)?;
    let group =
        to_pid(pgid).ok_or_else(|| ProcError::Spawn(io::Error::other("invalid group id")))?;
    let sig = match sig {
        GroupSignal::Term => Signal::TERM,
        GroupSignal::Kill => Signal::KILL,
    };
    kill_process_group(group, sig)
        .map_err(|e| ProcError::Spawn(io::Error::from_raw_os_error(e.raw_os_error())))
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    use super::*;

    fn sleeper() -> std::process::Child {
        Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap()
    }

    #[test]
    fn refuses_init_and_zero() {
        assert!(send_group(0, GroupSignal::Term).is_err());
        assert!(send_group(1, GroupSignal::Kill).is_err());
    }

    #[test]
    fn term_stops_a_group() {
        let mut child = sleeper();
        send_group(child.id(), GroupSignal::Term).unwrap();
        assert!(!child.wait().unwrap().success());
    }

    #[test]
    fn kill_stops_a_group() {
        let mut child = sleeper();
        send_group(child.id(), GroupSignal::Kill).unwrap();
        assert!(!child.wait().unwrap().success());
    }

    #[test]
    fn missing_group_is_an_error() {
        let mut child = sleeper();
        let pid = child.id();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(send_group(pid, GroupSignal::Term).is_err());
    }
}
