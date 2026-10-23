//! `moor front-shim` on macOS: the shim confines itself with nono's Seatbelt profile, which
//! starts from deny-all, and runs the front under it. The front may read the [`SYSTEM`] paths,
//! its program and its `read_only` paths, write only its state dir and `/dev/null`, and
//! connect to no Unix socket but its own and the DNS resolver's. It shares the shim's process
//! group, which the shim kills once the controller that started it is gone.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use nono::{AccessMode, CapabilitySet, Sandbox};
use rustix::process::{
    getpgrp, getpid, getppid, kill_process, kill_process_group, waitpid, Pid, Signal, WaitOptions,
};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

use super::{code, enforced, supported, Asked, Layout, POLL};

/// The host paths every front may read, where they exist: the programs, libraries, dyld
/// cache and configuration a front runs with. A path through a symlink, such as `/etc` to
/// `/private/etc`, may be read by either name.
pub(in crate::controller) const SYSTEM: [&str; 6] = [
    "/usr",
    "/bin",
    "/sbin",
    "/System",
    "/etc",
    "/var/run/resolv.conf",
];

/// The devices a front may read; it may also write `/dev/null`.
const DEVICES: [&str; 3] = ["/dev/zero", "/dev/random", "/dev/urandom"];

/// The socket macOS resolves names through.
const RESOLVER: &str = "/var/run/mDNSResponder";

/// Runs `moor front-shim` with `args`, the words after `front-shim`; returns the exit code.
///
/// # Errors
/// Returns why the front could not be confined or started.
pub fn main(args: &[OsString]) -> Result<u8, String> {
    let mut parts = args.splitn(2, |a| a == "--");
    let flags = parts.next().unwrap_or_default();
    let command = parts.next().unwrap_or_default();
    if let [flag] = flags {
        if flag == "--check" {
            confine(&Layout::default(), None)?;
            enforced()?;
            let said = "sandbox enforced: a write outside the allow-list was refused\n";
            let _ = io::Write::write_all(&mut io::stdout(), said.as_bytes());
            return Ok(0);
        }
    }
    let Asked { parent, layout } = Asked::parse(flags, true)?;
    let parent = parent.ok_or("--parent is required")?;
    if getppid() != Some(parent) {
        return Err(format!(
            "the controller (pid {}) is gone",
            parent.as_raw_nonzero()
        ));
    }
    let socket = std::env::var_os("MOOR_FRONT_SOCKET").map(PathBuf::from);
    confine(&layout, socket.as_deref())?;
    run(command, parent)
}

/// Confines this process and every process it starts to reading the [`SYSTEM`] paths, the
/// [`DEVICES`] and `layout`'s `read_only` paths, writing its `writable` dir and `/dev/null`,
/// and connecting to no Unix socket but `socket` and the [`RESOLVER`], each by the name given
/// or its real path. Other networking is left as it is.
fn confine(layout: &Layout, socket: Option<&Path>) -> Result<(), String> {
    supported(&Sandbox::support_info())?;
    let mut caps = CapabilitySet::new();
    let reads = SYSTEM
        .iter()
        .chain(&DEVICES)
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .chain(layout.read_only.iter().cloned());
    for path in reads {
        caps = allow(caps, &path, AccessMode::Read)?;
    }
    caps = allow(caps, Path::new("/dev/null"), AccessMode::ReadWrite)?;
    if let Some(dir) = &layout.writable {
        caps = allow(caps, dir, AccessMode::ReadWrite)?;
    }
    let mut dial = vec![];
    for path in std::iter::once(Path::new(RESOLVER)).chain(socket) {
        let real = fs::canonicalize(path)
            .map_err(|e| format!("cannot find the socket {}: {e}", path.display()))?;
        dial.push(path.to_owned());
        if real != path {
            dial.push(real);
        }
    }
    let rules = std::iter::once(r#"(deny network-outbound (subpath "/"))"#.to_owned()).chain(
        dial.iter()
            .map(|path| quoted(path).map(|p| format!("(allow network-outbound (path {p}))")))
            .collect::<Result<Vec<_>, _>>()?,
    );
    for rule in rules {
        caps.add_platform_rule(rule)
            .map_err(|e| format!("cannot build the sandbox: {e}"))?;
    }
    Sandbox::apply_auto(&caps).map_err(|e| format!("cannot apply the sandbox: {e}"))
}

/// Adds `path` to `caps` with `mode`, as a dir or a file as it is.
fn allow(caps: CapabilitySet, path: &Path, mode: AccessMode) -> Result<CapabilitySet, String> {
    let added = if path.is_dir() {
        caps.allow_path(path, mode)
    } else {
        caps.allow_file(path, mode)
    };
    added.map_err(|e| format!("cannot let the front reach {}: {e}", path.display()))
}

/// `path` as a Seatbelt string literal.
fn quoted(path: &Path) -> Result<String, String> {
    let text = path
        .to_str()
        .filter(|t| !t.chars().any(char::is_control))
        .ok_or_else(|| format!("{} cannot be named in the sandbox", path.display()))?;
    Ok(format!(
        "\"{}\"",
        text.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// Runs `command`, passes SIGTERM and SIGINT to it and reaps every child that exits; returns
/// the command's exit code once it exits, or 128 plus the signal that ended it. Once `parent`
/// is no longer this process's parent, kills this process group, the front with it, when this
/// process leads it, else the front alone.
fn run(command: &[OsString], parent: Pid) -> Result<u8, String> {
    let mut signals = Signals::new([SIGTERM, SIGINT]).map_err(|e| format!("signals: {e}"))?;
    let (program, args) = command.split_first().ok_or("no front command")?;
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", program.display()))?;
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
        if getppid() != Some(parent) {
            let _ = if getpgrp() == getpid() {
                kill_process_group(getpid(), Signal::KILL)
            } else {
                kill_process(pid, Signal::KILL)
            };
        }
        match waitpid(None, WaitOptions::NOHANG) {
            Ok(Some((reaped, status))) if reaped == pid => return Ok(code(status)),
            Ok(Some(_)) => {}
            Ok(None) => thread::sleep(POLL),
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::quoted;

    #[test]
    fn a_path_is_quoted_for_seatbelt() {
        assert_eq!(
            quoted(Path::new(r#"/a "b"\c"#)),
            Ok(r#""/a \"b\"\\c""#.to_owned())
        );
        assert!(quoted(Path::new("/a\nb")).is_err());
    }
}
