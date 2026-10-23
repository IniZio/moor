//! `moor front-shim`: runs a front in new user, mount, PID and network namespaces, inside a
//! root that holds only what a front needs.
//!
//! The controller runs `moor front-shim --parent PID [--proxy SOCKET] [--port PORT]...
//! [--writable DIR] [--read-only PATH]... -- COMMAND...`. That process dies with the
//! controller `PID` (it exits at once if `PID` is not its parent), starts the egress helper
//! and listens on the host's loopback for each PORT, enters the namespaces, serves the
//! front's proxy port and PORTs (see the `net` and `egress` modules) and runs
//! `moor front-shim --init` with the same paths as pid 1 of the new PID namespace. pid 1 builds the front's root (see the `root` module): the system dirs,
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

#[cfg(target_os = "linux")]
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
#[cfg(target_os = "linux")]
use std::thread;
use std::time::Duration;

#[cfg(target_os = "linux")]
use command_fds::{CommandFdExt as _, FdMapping};
use nono::SupportInfo;
#[cfg(target_os = "linux")]
use nono::{AccessMode, Sandbox};
#[cfg(target_os = "linux")]
use rustix::event::{poll, PollFd, PollFlags, Timespec};
#[cfg(target_os = "linux")]
use rustix::process::Pid;
use rustix::process::WaitStatus;
#[cfg(target_os = "linux")]
use rustix::process::{
    kill_process, set_dumpable_behavior, set_parent_process_death_signal, waitpid,
    DumpableBehavior, Signal, WaitOptions,
};
#[cfg(target_os = "linux")]
use rustix::thread::{set_capabilities, CapabilitySet, CapabilitySets};
#[cfg(target_os = "linux")]
use signal_hook::consts::{SIGINT, SIGTERM};
#[cfg(target_os = "linux")]
use signal_hook::iterator::Signals;

mod asked;
#[cfg(target_os = "linux")]
mod bridge;
#[cfg(target_os = "linux")]
mod denied;
#[cfg(target_os = "linux")]
mod egress;
#[cfg(target_os = "linux")]
mod enter;
#[cfg(target_os = "linux")]
mod net;
#[cfg(target_os = "linux")]
mod root;
#[cfg(target_os = "macos")]
mod seatbelt;

use asked::Asked;
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
            enter::isolate()?;
            enter::enter_pid_namespace()?;
            enter::pass_sys_admin()?;
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
        [flag, target] if flag == "--egress" => egress::helper(target),
        [flag, layout @ ..] if flag == "--init" => {
            let asked = Asked::parse(layout, false)?;
            init(&asked, command)
        }
        _ => outer(flags, command),
    }
}

/// The outer shim: dies with the controller, prepares the front's network from the host's
/// side, enters the namespaces and runs pid 1 with what `flags` ask.
#[cfg(target_os = "linux")]
fn outer(flags: &[OsString], command: &[OsString]) -> Result<u8, String> {
    let Asked {
        parent,
        proxy,
        layout,
        ports,
        ..
    } = Asked::parse(flags, true)?;
    enter::die_with(parent.ok_or("--parent is required")?)?;
    let outside = net::Outside::prepare(&ports)?;
    enter::isolate()?;
    let egress_port = outside.serve()?.to_string();
    enter::enter_pid_namespace()?;
    enter::pass_sys_admin()?;
    let (alive, _held) = std::io::pipe().map_err(|e| format!("pipe: {e}"))?;
    let proxy = proxy
        .iter()
        .flat_map(|p| [OsString::from("--proxy"), p.into()]);
    let writable = layout.writable.iter().map(|dir| ("--writable", dir));
    let read_only = layout.read_only.iter().map(|path| ("--read-only", path));
    relay(
        Command::new("/proc/self/exe")
            .args(["front-shim", "--init", "--egress-port", &egress_port])
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

/// What the front may reach beyond the system paths.
#[derive(Default)]
struct Layout {
    /// The dir the front may write; on Linux its `tmp` becomes the front's `/tmp`.
    writable: Option<PathBuf>,
    /// Files and dirs the front may read.
    read_only: Vec<PathBuf>,
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
/// confines itself to writing the writable dir and `/tmp`, drops every capability, turns
/// undumpable when it holds the Proxy socket's dir open, so the front cannot reach that dir
/// through `/proc/1/fd`, and runs
/// `command` with its socket on fd 3, `proxy` bridged to a loopback port, and the proxy
/// variables naming the egress proxy port when there is one. The kernel kills the namespace
/// if the outer shim dies. The outer shim holds the only write end of stdin, so a readable
/// stdin means it died before the death signal was set, and pid 1 exits.
#[cfg(target_os = "linux")]
fn init(asked: &Asked, command: &[OsString]) -> Result<u8, String> {
    let (layout, proxy) = (&asked.layout, asked.proxy.as_deref());
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
    if bridge.is_some() {
        set_dumpable_behavior(DumpableBehavior::NotDumpable)
            .map_err(|e| format!("cannot make pid 1 undumpable: {e}"))?;
    }
    let (program, args) = command.split_first().ok_or("no front command")?;
    let mut front = Command::new(program);
    front.args(args).stdin(Stdio::null());
    if let Some(bridge) = bridge {
        front.env(bridge::PORT_VAR, bridge.serve()?.to_string());
    }
    if let Some(port) = asked.egress_port {
        let url = format!("http://127.0.0.1:{port}");
        for var in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
            front.env(var, &url);
        }
        for var in ["NO_PROXY", "no_proxy"] {
            front.env(var, "localhost,127.0.0.1,::1");
        }
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
