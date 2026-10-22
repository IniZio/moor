//! `moor front-shim`: runs a front in new user, mount and PID namespaces, where the controller's
//! state dir, operator socket and vault key are out of reach.
//!
//! The controller runs `moor front-shim [--cover DIR | --mask FILE]... -- COMMAND...`. That
//! process enters the namespaces, covers each DIR with an empty tmpfs and each FILE with
//! `/dev/null`, then runs `moor front-shim --init -- COMMAND...` as pid 1 of the new PID
//! namespace. pid 1 mounts a fresh `/proc`, so no controller pid is visible, and runs the front
//! without capabilities, so the front cannot unmount what hides those paths. Both shims pass
//! SIGTERM and SIGINT on, reap every child and exit with the front's status. `--check` only
//! enters the namespaces, so the controller can refuse fronts on a host that forbids them.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

use nix::sched::{unshare, CloneFlags};
use rustix::mount::{mount, mount_bind, mount_change, MountFlags, MountPropagationFlags};
use rustix::process::{
    getgid, getuid, kill_process, set_parent_process_death_signal, waitpid, Pid, Signal,
    WaitOptions, WaitStatus,
};
use rustix::thread::{
    capabilities, configure_capability_in_ambient_set, set_capabilities, CapabilitySet,
    CapabilitySets,
};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

/// How often a shim checks for exited children and signals to pass on.
const POLL: Duration = Duration::from_millis(50);

/// Runs `moor front-shim` with `args`, the words after `front-shim`; returns the exit code.
///
/// # Errors
/// Returns why the front could not be isolated or started.
pub fn main(args: &[OsString]) -> Result<u8, String> {
    let mut parts = args.splitn(2, |a| a == "--");
    let flags = parts.next().unwrap_or_default();
    let command = parts.next().unwrap_or_default();
    match flags {
        [flag] if flag == "--check" => isolate(&[], &[]).map(|()| 0),
        [flag] if flag == "--init" => init(command),
        _ => {
            let (cover, mask) = hidden(flags)?;
            let exe = std::env::current_exe().map_err(|e| format!("no own executable: {e}"))?;
            isolate(&cover, &mask)?;
            let mut caps = capabilities(None).map_err(|e| format!("capget: {e}"))?;
            caps.inheritable |= CapabilitySet::SYS_ADMIN;
            set_capabilities(None, caps).map_err(|e| format!("capset: {e}"))?;
            configure_capability_in_ambient_set(CapabilitySet::SYS_ADMIN, true)
                .map_err(|e| format!("cannot pass CAP_SYS_ADMIN to pid 1: {e}"))?;
            relay(
                Command::new(exe)
                    .args(["front-shim", "--init", "--"])
                    .args(command),
            )
        }
    }
}

/// Splits `--cover DIR` and `--mask FILE` flags into the dirs and the files to hide.
fn hidden(flags: &[OsString]) -> Result<(Vec<PathBuf>, Vec<PathBuf>), String> {
    let (mut cover, mut mask) = (Vec::new(), Vec::new());
    let mut flags = flags.iter();
    while let Some(flag) = flags.next() {
        let into = match flag.to_str() {
            Some("--cover") => &mut cover,
            Some("--mask") => &mut mask,
            _ => return Err(format!("unknown flag {}", flag.display())),
        };
        let path = flags
            .next()
            .ok_or_else(|| format!("{} needs a path", flag.display()))?;
        into.push(PathBuf::from(path));
    }
    Ok((cover, mask))
}

/// Enters new user, mount and PID namespaces as the same uid and gid, then hides `cover` under
/// empty read-only tmpfs mounts and `mask` under `/dev/null`. The PID namespace applies to the
/// next child, which becomes its pid 1.
fn isolate(cover: &[PathBuf], mask: &[PathBuf]) -> Result<(), String> {
    let (uid, gid) = (getuid().as_raw(), getgid().as_raw());
    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWPID)
        .map_err(|e| format!("cannot create user, mount and PID namespaces: {e}"))?;
    for (file, text) in [
        ("setgroups", "deny".to_owned()),
        ("uid_map", format!("{uid} {uid} 1")),
        ("gid_map", format!("{gid} {gid} 1")),
    ] {
        fs::write(format!("/proc/self/{file}"), text)
            .map_err(|e| format!("cannot write /proc/self/{file}: {e}"))?;
    }
    mount_change(
        "/",
        MountPropagationFlags::PRIVATE | MountPropagationFlags::REC,
    )
    .map_err(|e| format!("cannot make mounts private: {e}"))?;
    for file in mask {
        mount_bind("/dev/null", file)
            .map_err(|e| format!("cannot mask {}: {e}", file.display()))?;
    }
    let flags = MountFlags::RDONLY | MountFlags::NOSUID | MountFlags::NODEV | MountFlags::NOEXEC;
    for dir in cover {
        mount("tmpfs", dir, "tmpfs", flags, None)
            .map_err(|e| format!("cannot cover {}: {e}", dir.display()))?;
    }
    Ok(())
}

/// pid 1 of the front's PID namespace: mounts its `/proc`, drops every capability and runs
/// `command`. The kernel kills the namespace if the outer shim dies.
fn init(command: &[OsString]) -> Result<u8, String> {
    set_parent_process_death_signal(Some(Signal::KILL))
        .map_err(|e| format!("cannot set the parent death signal: {e}"))?;
    let flags = MountFlags::NOSUID | MountFlags::NODEV | MountFlags::NOEXEC;
    mount("proc", "/proc", "proc", flags, None).map_err(|e| format!("cannot mount /proc: {e}"))?;
    let none = CapabilitySet::empty();
    set_capabilities(
        None,
        CapabilitySets {
            effective: none,
            permitted: none,
            inheritable: none,
        },
    )
    .map_err(|e| format!("cannot drop capabilities: {e}"))?;
    let (program, args) = command.split_first().ok_or("no front command")?;
    relay(Command::new(program).args(args))
}

/// Runs `command`, passes SIGTERM and SIGINT to it and reaps every child that exits; returns
/// the command's exit code once it exits, or 128 plus the signal that ended it.
fn relay(command: &mut Command) -> Result<u8, String> {
    let mut signals = Signals::new([SIGTERM, SIGINT]).map_err(|e| format!("signals: {e}"))?;
    let child = command
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", command.get_program().display()))?;
    let pid = Pid::from_child(&child);
    loop {
        for signal in signals.pending() {
            let signal = if signal == SIGTERM {
                Signal::TERM
            } else {
                Signal::INT
            };
            let _ = kill_process(pid, signal);
        }
        match waitpid(None, WaitOptions::NOHANG) {
            Ok(Some((reaped, status))) if reaped == pid => return Ok(code(status)),
            Ok(Some(_)) => {}
            Ok(None) => thread::sleep(POLL),
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
}

/// The shell-style exit code of `status`.
fn code(status: WaitStatus) -> u8 {
    let code = status
        .exit_status()
        .or_else(|| status.terminating_signal().map(|s| 128 + s))
        .unwrap_or(1);
    u8::try_from(code).unwrap_or(1)
}
