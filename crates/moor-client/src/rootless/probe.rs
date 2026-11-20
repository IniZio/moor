//! Reads what the preflight checks, and asks Docker whether it is rootless.

use std::io::{Read as _, Write as _};
use std::net::Ipv4Addr;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::{child_ip_from, cmdline_words, is_rootless, state_dir, SECURITY_ARGS};

/// The programs a rootless daemon, and a relay, need on `PATH`.
const BINARIES: [&str; 4] = ["newuidmap", "newgidmap", "slirp4netns", "nsenter"];
/// The sysctl that stops unprivileged user namespaces under `AppArmor`.
const APPARMOR_SYSCTL: &str = "/proc/sys/kernel/apparmor_restrict_unprivileged_userns";
/// The host's KVM device.
const KVM: &str = "/dev/kvm";

/// What the host says, before any check reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The user's name, else the uid.
    pub user: String,
    /// The user's uid.
    pub uid: u32,
    /// `/etc/subuid`, when it can be read.
    pub subuid: Option<String>,
    /// `/etc/subgid`, when it can be read.
    pub subgid: Option<String>,
    /// The programs of `BINARIES` that are not on `PATH`.
    pub missing: Vec<String>,
    /// The `AppArmor` sysctl's value, when the host has it.
    pub apparmor_restrict: Option<String>,
    /// What `systemctl --user is-enabled docker` printed.
    pub unit: Option<String>,
    /// Whether the user lingers, when `loginctl` says.
    pub linger: Option<bool>,
    /// The arguments rootlesskit runs with, else why they could not be read.
    pub rootlesskit: Result<Vec<String>, String>,
    /// The permission bits of `/dev/kvm`, when the host has it.
    pub kvm_mode: Option<u32>,
}

impl Facts {
    /// Reads the host for the user running this process; `path` is the `PATH` to look for
    /// programs on, and `user` the `USER` the environment names.
    #[must_use]
    pub fn read(path: Option<&str>, user: Option<&str>) -> Self {
        let uid = std::fs::metadata("/proc/self").map_or(0, |m| m.uid());
        let user = user
            .filter(|u| !u.is_empty())
            .map(str::to_owned)
            .or_else(|| run("id", &["-un"]))
            .unwrap_or_else(|| uid.to_string());
        Self {
            linger: lingers(&user),
            subuid: std::fs::read_to_string("/etc/subuid").ok(),
            subgid: std::fs::read_to_string("/etc/subgid").ok(),
            missing: BINARIES
                .iter()
                .filter(|b| !on_path(path, b))
                .map(|b| (*b).to_owned())
                .collect(),
            apparmor_restrict: std::fs::read_to_string(APPARMOR_SYSCTL).ok(),
            unit: run("systemctl", &["--user", "is-enabled", "docker"]),
            rootlesskit: rootlesskit_args(),
            kvm_mode: std::fs::metadata(KVM).ok().map(|m| m.mode() & 0o777),
            user,
            uid,
        }
    }
}

/// Whether the daemon `docker` talks to is rootless; a docker that does not answer is not.
#[must_use]
pub fn detect() -> bool {
    run("docker", &SECURITY_ARGS).is_some_and(|out| is_rootless(&out))
}

/// The trimmed stdout of `program`, whatever its exit code; none when it cannot run.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Whether `bin` is an executable file in a directory of `path`.
fn on_path(path: Option<&str>, bin: &str) -> bool {
    path.into_iter()
        .flat_map(std::env::split_paths)
        .any(|d| std::fs::metadata(d.join(bin)).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0))
}

/// Whether the user lingers; unknown when `loginctl` does not say.
fn lingers(user: &str) -> Option<bool> {
    let out = run("loginctl", &["show-user", user, "--property=Linger"])?;
    match out.as_str() {
        "Linger=yes" => Some(true),
        "Linger=no" => Some(false),
        _ => None,
    }
}

/// The state dir of the rootlesskit that runs the daemon `docker` talks to.
///
/// # Errors
/// The daemon's address cannot be read, or is not a Unix socket.
pub fn daemon_state() -> Result<PathBuf, String> {
    let host = run(
        "docker",
        &[
            "context",
            "inspect",
            "--format",
            "{{.Endpoints.docker.Host}}",
        ],
    )
    .filter(|h| !h.is_empty())
    .ok_or("cannot read the address of the Docker daemon with `docker context inspect`")?;
    state_dir(&host)
}

/// The address of the namespace of the rootlesskit whose state dir is `state`, which its `api.sock` reports.
///
/// # Errors
/// The socket does not answer, or its network driver names no address.
pub fn child_ip(state: &Path) -> Result<Ipv4Addr, String> {
    let sock = state.join("api.sock");
    let say = |e: std::io::Error| format!("cannot ask rootlesskit at {}: {e}", sock.display());
    let mut conn = UnixStream::connect(&sock).map_err(say)?;
    let wait = Some(Duration::from_secs(5));
    conn.set_read_timeout(wait).map_err(say)?;
    conn.write_all(b"GET /v1/info HTTP/1.0\r\nHost: rootlesskit\r\n\r\n")
        .map_err(say)?;
    let mut reply = String::new();
    conn.read_to_string(&mut reply).map_err(say)?;
    let body = reply.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    child_ip_from(body)
}

/// The arguments of the rootlesskit that runs the daemon `docker` talks to. `docker context
/// inspect` names the daemon's socket, whose directory holds rootlesskit's state dir, where
/// `child_pid` names rootlesskit's child; the child's `/proc` command line repeats the flags.
fn rootlesskit_args() -> Result<Vec<String>, String> {
    let state = daemon_state()?;
    let pid_file = state.join("child_pid");
    let pid = std::fs::read_to_string(&pid_file)
        .map_err(|e| format!("cannot read {}: {e}", pid_file.display()))?;
    let cmdline = Path::new("/proc").join(pid.trim()).join("cmdline");
    std::fs::read(&cmdline)
        .map(|raw| cmdline_words(&raw))
        .map_err(|e| format!("cannot read {}: {e}", cmdline.display()))
}
