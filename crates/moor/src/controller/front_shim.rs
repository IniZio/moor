//! `moor front-shim`: runs a front in new user, mount and PID namespaces, where the controller's
//! state dir, operator socket and vault key are out of reach.
//!
//! The controller runs `moor front-shim --parent PID [--cover DIR | --mask FILE]... -- COMMAND...`.
//! That process dies with the controller `PID` (it exits at once if `PID` is not its parent),
//! enters the namespaces, covers each DIR with an empty tmpfs and each FILE with `/dev/null`,
//! then runs `moor front-shim --init -- COMMAND...` as pid 1 of the new PID namespace. pid 1
//! mounts a fresh `/proc`, so no controller pid is visible, moves to `/`, so no cwd reaches
//! the covered paths, and runs the front without capabilities, so the front cannot unmount what
//! hides those paths. Both shims pass SIGTERM and SIGINT on, reap every child and exit with the
//! front's status. `--check` only enters the namespaces, so the controller can refuse fronts on
//! a host that forbids them.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use nix::sched::{unshare, CloneFlags};
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::mount::{mount, mount_bind, mount_change, MountFlags, MountPropagationFlags};
use rustix::process::{
    getgid, getppid, getuid, kill_process, set_parent_process_death_signal, waitpid, Pid, Signal,
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
            let Hidden {
                parent,
                cover,
                mask,
            } = Hidden::parse(flags)?;
            die_with(parent)?;
            let exe = std::env::current_exe().map_err(|e| format!("no own executable: {e}"))?;
            isolate(&cover, &mask)?;
            let mut caps = capabilities(None).map_err(|e| format!("capget: {e}"))?;
            caps.inheritable |= CapabilitySet::SYS_ADMIN;
            set_capabilities(None, caps).map_err(|e| format!("capset: {e}"))?;
            configure_capability_in_ambient_set(CapabilitySet::SYS_ADMIN, true)
                .map_err(|e| format!("cannot pass CAP_SYS_ADMIN to pid 1: {e}"))?;
            let (alive, _held) = std::io::pipe().map_err(|e| format!("pipe: {e}"))?;
            relay(
                Command::new(exe)
                    .args(["front-shim", "--init", "--"])
                    .args(command)
                    .stdin(alive),
            )
        }
    }
}

/// Makes this process die with `parent`, the controller that started it: the kernel kills it
/// when the controller exits, and it exits now if the controller already did.
fn die_with(parent: Pid) -> Result<(), String> {
    set_parent_process_death_signal(Some(Signal::KILL))
        .map_err(|e| format!("cannot set the parent death signal: {e}"))?;
    if getppid() != Some(parent) {
        return Err(format!(
            "the controller (pid {}) is gone",
            parent.as_raw_nonzero()
        ));
    }
    Ok(())
}

/// What `moor front-shim` was asked to keep a front away from, and the controller it serves.
struct Hidden {
    /// The controller that started this shim.
    parent: Pid,
    /// Dirs to cover with an empty tmpfs.
    cover: Vec<PathBuf>,
    /// Files to replace with `/dev/null`.
    mask: Vec<PathBuf>,
}

impl Hidden {
    /// Reads `--parent PID`, `--cover DIR` and `--mask FILE` flags.
    fn parse(flags: &[OsString]) -> Result<Self, String> {
        let (mut parent, mut cover, mut mask) = (None, Vec::new(), Vec::new());
        let mut flags = flags.iter();
        while let Some(flag) = flags.next() {
            let word = flags
                .next()
                .ok_or_else(|| format!("{} needs a value", flag.display()))?;
            match flag.to_str() {
                Some("--parent") => {
                    parent = word
                        .to_str()
                        .and_then(|w| w.parse().ok())
                        .and_then(Pid::from_raw);
                    if parent.is_none() {
                        return Err(format!("--parent {} is not a pid", word.display()));
                    }
                }
                Some("--cover") => cover.push(PathBuf::from(word)),
                Some("--mask") => mask.push(PathBuf::from(word)),
                _ => return Err(format!("unknown flag {}", flag.display())),
            }
        }
        let parent = parent.ok_or("--parent is required")?;
        Ok(Self {
            parent,
            cover,
            mask,
        })
    }
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

/// pid 1 of the front's PID namespace: mounts its `/proc`, moves to `/`, drops every capability
/// and runs `command`. The kernel kills the namespace if the outer shim dies. The outer shim
/// holds the only write end of stdin, so a readable stdin means it died before the death signal
/// was set, and pid 1 exits.
fn init(command: &[OsString]) -> Result<u8, String> {
    set_parent_process_death_signal(Some(Signal::KILL))
        .map_err(|e| format!("cannot set the parent death signal: {e}"))?;
    let held = io::stdin();
    let mut outer = [PollFd::new(&held, PollFlags::IN)];
    let idle = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if poll(&mut outer, Some(&idle)).map_err(|e| format!("poll: {e}"))? > 0 {
        return Err("the outer shim exited before pid 1 started".to_owned());
    }
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
    std::env::set_current_dir("/").map_err(|e| format!("cannot move to /: {e}"))?;
    let (program, args) = command.split_first().ok_or("no front command")?;
    relay(Command::new(program).args(args).stdin(Stdio::null()))
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
