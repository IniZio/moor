//! `moor front-shim`: runs a front in new user, mount and PID namespaces, inside a root that
//! holds only what a front needs.
//!
//! The controller runs `moor front-shim --parent PID [--proxy SOCKET] [--writable DIR]
//! [--read-only PATH]... -- COMMAND...`. That process dies with the controller `PID` (it exits at once if `PID` is not
//! its parent), enters the namespaces and runs `moor front-shim --init` with the same paths as
//! pid 1 of the new PID namespace. pid 1 builds the front's root (see the `root` module): the system dirs,
//! a few devices, the program and each PATH read-only, DIR read-write with its `tmp` as `/tmp`,
//! and a fresh `/proc`, so no controller pid is visible. It pivots into that root, confines
//! itself with nono's sandbox so that nothing but DIR, `/tmp` and `/dev/null` can be written,
//! and runs the front without capabilities. Both shims pass SIGTERM and SIGINT on, reap every
//! child and exit with the front's status. `--check` enters the namespaces, then as pid 1
//! (`--check-root`) the root and the sandbox, so the controller can refuse fronts on a host
//! that forbids any of them. With `--proxy`, pid 1 also bridges a loopback port to the front
//! Proxy's SOCKET and tells the front the port (see the `bridge` module).
//!
//! macOS has none of these namespaces: there the shim confines itself and the front with
//! nono's Seatbelt profile instead (see the `seatbelt` module), and no front gets a Proxy.

use std::ffi::OsString;
use std::fs;
use std::io;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
#[cfg(target_os = "linux")]
use std::thread;
use std::time::Duration;

#[cfg(target_os = "linux")]
use command_fds::{CommandFdExt as _, FdMapping};
#[cfg(target_os = "linux")]
use nix::sched::{unshare, CloneFlags};
use nono::SupportInfo;
#[cfg(target_os = "linux")]
use nono::{AccessMode, Sandbox};
#[cfg(target_os = "linux")]
use rustix::event::{poll, PollFd, PollFlags, Timespec};
#[cfg(target_os = "linux")]
use rustix::mount::{mount_change, MountPropagationFlags};
#[cfg(target_os = "linux")]
use rustix::process::{
    getgid, getppid, getuid, kill_process, set_parent_process_death_signal, waitpid, Signal,
    WaitOptions,
};
use rustix::process::{Pid, WaitStatus};
#[cfg(target_os = "linux")]
use rustix::thread::{
    capabilities, configure_capability_in_ambient_set, set_capabilities, CapabilitySet,
    CapabilitySets,
};
#[cfg(target_os = "linux")]
use signal_hook::consts::{SIGINT, SIGTERM};
#[cfg(target_os = "linux")]
use signal_hook::iterator::Signals;

#[cfg(target_os = "linux")]
mod bridge;
#[cfg(target_os = "linux")]
mod root;
#[cfg(target_os = "macos")]
mod seatbelt;

#[cfg(target_os = "linux")]
pub(super) use root::{refuse_under_tmp, SYSTEM};
#[cfg(target_os = "macos")]
pub use seatbelt::main;
#[cfg(target_os = "macos")]
pub(super) use seatbelt::SYSTEM;

/// How often a shim checks for exited children and signals to pass on.
const POLL: Duration = Duration::from_millis(50);

/// Runs `moor front-shim` with `args`, the words after `front-shim`; returns the exit code.
///
/// # Errors
/// Returns why the front could not be isolated or started.
#[cfg(target_os = "linux")]
pub fn main(args: &[OsString]) -> Result<u8, String> {
    let mut parts = args.splitn(2, |a| a == "--");
    let flags = parts.next().unwrap_or_default();
    let command = parts.next().unwrap_or_default();
    match flags {
        [flag] if flag == "--check" => {
            isolate()?;
            pass_sys_admin()?;
            relay(
                Command::new("/proc/self/exe")
                    .args(["front-shim", "--check-root"])
                    .stdin(Stdio::null()),
            )
        }
        [flag] if flag == "--check-root" => root::enter(&Layout::default())
            .and_then(|_| confine(&[]))
            .and_then(|()| enforced())
            .map(|()| {
                let said = "sandbox enforced: a write outside the allow-list was refused\n";
                let _ = io::Write::write_all(&mut io::stdout(), said.as_bytes());
                0
            }),
        [flag, layout @ ..] if flag == "--init" => {
            let asked = Asked::parse(layout, false)?;
            init(&asked.layout, asked.proxy.as_deref(), command)
        }
        _ => {
            let Asked {
                parent,
                proxy,
                layout,
            } = Asked::parse(flags, true)?;
            die_with(parent.ok_or("--parent is required")?)?;
            isolate()?;
            pass_sys_admin()?;
            let (alive, _held) = std::io::pipe().map_err(|e| format!("pipe: {e}"))?;
            let proxy = proxy
                .iter()
                .flat_map(|p| [OsString::from("--proxy"), p.into()]);
            let writable = layout.writable.iter().map(|dir| ("--writable", dir));
            let read_only = layout.read_only.iter().map(|path| ("--read-only", path));
            relay(
                Command::new("/proc/self/exe")
                    .args(["front-shim", "--init"])
                    .args(proxy)
                    .args(
                        writable
                            .chain(read_only)
                            .flat_map(|(flag, path)| [OsString::from(flag), path.into()]),
                    )
                    .arg("--")
                    .args(command)
                    .stdin(alive),
            )
        }
    }
}

/// Keeps `CAP_SYS_ADMIN` in the namespaces for the next program this process runs, pid 1,
/// which mounts the front's root.
#[cfg(target_os = "linux")]
fn pass_sys_admin() -> Result<(), String> {
    let mut caps = capabilities(None).map_err(|e| format!("capget: {e}"))?;
    caps.inheritable |= CapabilitySet::SYS_ADMIN;
    set_capabilities(None, caps).map_err(|e| format!("capset: {e}"))?;
    configure_capability_in_ambient_set(CapabilitySet::SYS_ADMIN, true)
        .map_err(|e| format!("cannot pass CAP_SYS_ADMIN to pid 1: {e}"))
}

/// Makes this process die with `parent`, the controller that started it: the kernel kills it
/// when the controller exits, and it exits now if the controller already did.
#[cfg(target_os = "linux")]
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

/// What the front may reach beyond the system paths.
#[derive(Default)]
struct Layout {
    /// The dir the front may write; on Linux its `tmp` becomes the front's `/tmp`.
    writable: Option<PathBuf>,
    /// Files and dirs the front may read.
    read_only: Vec<PathBuf>,
}

/// What `moor front-shim` was asked: the controller it serves and the front's root.
struct Asked {
    /// The controller that started this shim; pid 1 is not told.
    parent: Option<Pid>,
    /// The front Proxy's socket, which pid 1 bridges to a loopback port.
    proxy: Option<PathBuf>,
    /// What the front's root holds.
    layout: Layout,
}

impl Asked {
    /// Reads `--writable DIR` and `--proxy SOCKET` (at most one each) and `--read-only PATH`
    /// flags, and `--parent PID` when `outer`.
    fn parse(flags: &[OsString], outer: bool) -> Result<Self, String> {
        let mut asked = Self {
            parent: None,
            proxy: None,
            layout: Layout::default(),
        };
        let mut flags = flags.iter();
        while let Some(flag) = flags.next() {
            let word = flags
                .next()
                .ok_or_else(|| format!("{} needs a value", flag.display()))?;
            match flag.to_str() {
                Some("--parent") if outer => {
                    asked.parent = word
                        .to_str()
                        .and_then(|w| w.parse().ok())
                        .and_then(Pid::from_raw);
                    if asked.parent.is_none() {
                        return Err(format!("--parent {} is not a pid", word.display()));
                    }
                }
                Some("--proxy") if asked.proxy.is_none() => {
                    asked.proxy = Some(PathBuf::from(word));
                }
                Some("--writable") if asked.layout.writable.is_none() => {
                    asked.layout.writable = Some(PathBuf::from(word));
                }
                Some("--read-only") => asked.layout.read_only.push(PathBuf::from(word)),
                _ => return Err(format!("unknown flag {}", flag.display())),
            }
        }
        Ok(asked)
    }
}

/// Enters new user, mount and PID namespaces as the same uid and gid, with mounts that no
/// longer reach the host's. The PID namespace applies to the next child, which becomes its
/// pid 1.
#[cfg(target_os = "linux")]
fn isolate() -> Result<(), String> {
    let (uid, gid) = (getuid().as_raw(), getgid().as_raw());
    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWPID)
        .map_err(|e| {
            format!(
                "they need unprivileged user namespaces (sysctl kernel.unprivileged_userns_clone=1, \
                 user.max_user_namespaces above 0, kernel.apparmor_restrict_unprivileged_userns=0): \
                 cannot create user, mount and PID namespaces: {e}"
            )
        })?;
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
    .map_err(|e| format!("cannot make mounts private: {e}"))
}

/// Fails unless the sandbox now in force refuses a write it would refuse a front: opening
/// `/dev/zero` for writing, which a read-only mount alone allows.
fn enforced() -> Result<(), String> {
    match fs::OpenOptions::new().write(true).open("/dev/zero") {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Ok(()),
        Err(e) => Err(format!("the sandbox check failed: {e}")),
        Ok(_) => Err("the sandbox did not take effect".to_owned()),
    }
}

/// Confines this process and every process it starts: each may read and run anything, but
/// write only `writable` and `/dev/null`.
#[cfg(target_os = "linux")]
fn confine(writable: &[PathBuf]) -> Result<(), String> {
    supported(&Sandbox::support_info())?;
    let mut caps = nono::CapabilitySet::new()
        .allow_path("/", AccessMode::Read)
        .and_then(|caps| caps.allow_file("/dev/null", AccessMode::ReadWrite))
        .map_err(|e| format!("cannot build the sandbox: {e}"))?;
    for dir in writable {
        caps = caps
            .allow_path(dir, AccessMode::ReadWrite)
            .map_err(|e| format!("cannot let the front write {}: {e}", dir.display()))?;
    }
    Sandbox::apply_auto(&caps)
        .map(drop)
        .map_err(|e| format!("cannot apply the sandbox: {e}"))
}

/// Refuses a host where nono cannot sandbox a front.
fn supported(info: &SupportInfo) -> Result<(), String> {
    if info.is_supported {
        return Ok(());
    }
    Err(format!(
        "they need nono's sandbox, which this {} host lacks: {}",
        info.platform, info.details
    ))
}

/// pid 1 of the front's PID namespace: moves into the front's root as `layout` describes,
/// confines itself to writing the writable dir and `/tmp`, drops every capability and runs
/// `command` with its socket on fd 3, and with `proxy` bridged to a loopback port. The kernel kills the namespace if the outer shim dies. The
/// outer shim holds the only write end of stdin, so a readable stdin means it died before the
/// death signal was set, and pid 1 exits.
#[cfg(target_os = "linux")]
fn init(layout: &Layout, proxy: Option<&Path>, command: &[OsString]) -> Result<u8, String> {
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
    let bridge = proxy.map(bridge::Bridge::open).transpose()?;
    let socket = root::enter(layout)?;
    let _ = nix::unistd::close(3);
    let writable: Vec<PathBuf> = layout
        .writable
        .iter()
        .cloned()
        .chain(layout.writable.as_ref().map(|_| PathBuf::from("/tmp")))
        .collect();
    confine(&writable)?;
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
    let mut front = Command::new(program);
    front.args(args).stdin(Stdio::null());
    if let Some(bridge) = bridge {
        front.env(bridge::PORT_VAR, bridge.serve()?.to_string());
    }
    if let Some(fd) = socket {
        front
            .fd_mappings(vec![FdMapping {
                parent_fd: fd,
                child_fd: 3,
            }])
            .map_err(|e| format!("cannot pass the front its socket: {e}"))?;
    }
    relay(&mut front)
}

/// Runs `command`, passes SIGTERM and SIGINT to it and reaps every child that exits; returns
/// the command's exit code once it exits, or 128 plus the signal that ended it.
#[cfg(target_os = "linux")]
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

#[cfg(test)]
mod tests {
    use nono::SupportInfo;

    use super::supported;

    #[test]
    fn a_host_without_the_sandbox_is_refused_with_why() {
        let info = SupportInfo {
            is_supported: false,
            platform: "linux",
            details: "Landlock not available".to_owned(),
        };
        let e = supported(&info).unwrap_err();
        assert!(e.contains("nono's sandbox"), "{e}");
        assert!(e.contains("Landlock not available"), "{e}");
        let info = SupportInfo {
            is_supported: true,
            ..info
        };
        assert_eq!(supported(&info), Ok(()));
    }

    #[test]
    fn a_mac_without_seatbelt_is_refused_as_linux_is() {
        let info = SupportInfo {
            is_supported: false,
            platform: "macos",
            details: "Seatbelt not available".to_owned(),
        };
        assert_eq!(
            supported(&info),
            Err(
                "they need nono's sandbox, which this macos host lacks: Seatbelt not available"
                    .to_owned()
            )
        );
    }
}
