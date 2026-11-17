//! The controller as a systemd user service: a socket unit that holds the controller socket
//! across restarts, and the service it starts. Both belong to the person, so no root.

use std::fmt::Write as _;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::files::{write, Layout};
use crate::doctor::Check;
use crate::env::Env;

/// The service unit's name.
pub(crate) const SERVICE: &str = "moor.service";
/// The socket unit's name.
pub(crate) const SOCKET: &str = "moor.socket";
/// The machine runner's unit.
pub(crate) const RUNNER: &str = "moor-runner.service";
/// The local node's machine agent's unit.
pub(crate) const AGENT: &str = "moor-agent.service";

/// What the service runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unit {
    /// The `moor` executable.
    pub exe: PathBuf,
    /// The controller config it serves.
    pub config: PathBuf,
    /// The controller socket the socket unit holds.
    pub socket: PathBuf,
    /// Environment the controller needs beside the service manager's own.
    pub env: Vec<(String, String)>,
}

impl Unit {
    /// The unit for `layout`, run by this executable with this `PATH`, whose Proxy also listens
    /// on `egress`, the address the Boxes of Docker machines on this host dial.
    pub(crate) fn new(env: &Env, layout: &Layout, egress: SocketAddr) -> Result<Self, Check> {
        let exe = std::env::current_exe()
            .map_err(|e| Check::fail("service", format!("cannot find moor itself: {e}"), ""))?;
        let mut vars = vec![
            (
                "MOOR_CONFIG".to_owned(),
                layout.user_config.display().to_string(),
            ),
            ("MOOR_EGRESS_LISTEN".to_owned(), egress.to_string()),
        ];
        if let Some(path) = env.get("PATH") {
            vars.push(("PATH".to_owned(), path.to_owned()));
        }
        Ok(Self {
            exe,
            config: layout.controller_config.clone(),
            socket: layout.socket.clone(),
            env: vars,
        })
    }
}

/// `word` as one quoted systemd word: `%` is a specifier there, so it is doubled.
fn quoted(word: &str) -> String {
    let escaped = word
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!("\"{escaped}\"")
}

/// The socket unit.
pub(crate) fn socket_unit(unit: &Unit) -> String {
    format!(
        "[Unit]
Description=moor controller socket

[Socket]
ListenStream={}
FileDescriptorName=controller
Service={SERVICE}
SocketMode=0600
DirectoryMode=0700

[Install]
WantedBy=sockets.target
",
        unit.socket.display().to_string().replace('%', "%%")
    )
}

/// The service unit.
pub(crate) fn service_unit(unit: &Unit) -> String {
    let exec = [
        unit.exe.display().to_string(),
        "controller".to_owned(),
        "serve".to_owned(),
        "--config".to_owned(),
        unit.config.display().to_string(),
    ]
    .iter()
    .map(|w| quoted(w))
    .collect::<Vec<_>>()
    .join(" ");
    let mut env = String::new();
    for (k, v) in &unit.env {
        let _ = writeln!(env, "Environment={}", quoted(&format!("{k}={v}")));
    }
    format!(
        "[Unit]
Description=moor controller
Requires={SOCKET}
After={SOCKET}

[Service]
ExecStart={exec}
{env}KillMode=process
TimeoutStopSec=660
Restart=on-failure

[Install]
WantedBy=default.target
"
    )
}

/// A unit that keeps `exe args` running, started after `after` and restarted when it ends.
pub(crate) fn process_unit(
    description: &str,
    after: &str,
    exe: &Path,
    args: &[String],
    env: &[(String, String)],
) -> String {
    let exec = std::iter::once(exe.display().to_string())
        .chain(args.iter().cloned())
        .map(|w| quoted(&w))
        .collect::<Vec<_>>()
        .join(" ");
    let mut vars = String::new();
    for (k, v) in env {
        let _ = writeln!(vars, "Environment={}", quoted(&format!("{k}={v}")));
    }
    format!(
        "[Unit]
Description={description}
Wants={after}
After={after}

[Service]
ExecStart={exec}
{vars}Restart=always
RestartSec=2

[Install]
WantedBy=default.target
"
    )
}

/// Where user units go: `$XDG_CONFIG_HOME/systemd/user`, else `~/.config/systemd/user`.
pub(crate) fn unit_dir(env: &Env) -> Option<PathBuf> {
    env.xdg("XDG_CONFIG_HOME", ".config", "systemd/user")
}

/// Writes both units, enables them and starts the service: anew when its unit changed.
pub(crate) fn install(dir: &Path, unit: &Unit) -> Result<Check, Check> {
    let files = [
        (dir.join(SOCKET), socket_unit(unit)),
        (dir.join(SERVICE), service_unit(unit)),
    ];
    let mut changed = false;
    for (path, text) in &files {
        if fs::read_to_string(path).ok().as_deref() != Some(text.as_str()) {
            write(path, text)?;
            changed = true;
        }
    }
    if changed {
        systemctl(&["daemon-reload"])?;
    }
    systemctl(&["enable", SOCKET, SERVICE])?;
    systemctl(&["start", SOCKET])?;
    systemctl(&[if changed { "restart" } else { "start" }, SERVICE])?;
    Ok(Check::ok(
        "service",
        format!("{SOCKET} and {SERVICE} enabled in {}", dir.display()),
    ))
}

/// Writes the unit `name`, enables it and starts it: anew when its text changed or `restart` is
/// set. Whether it changed.
pub(crate) fn install_one(
    dir: &Path,
    name: &str,
    text: &str,
    restart: bool,
) -> Result<bool, Check> {
    let path = dir.join(name);
    let changed = fs::read_to_string(&path).ok().as_deref() != Some(text);
    if changed {
        write(&path, text)?;
        systemctl(&["daemon-reload"])?;
    }
    systemctl(&["enable", name])?;
    systemctl(&[
        if changed || restart {
            "restart"
        } else {
            "start"
        },
        name,
    ])?;
    Ok(changed)
}

/// Stops and disables every unit `up` wrote, the agent first, and removes their files; whether
/// any was there.
pub(crate) fn remove(dir: &Path) -> Result<bool, Check> {
    let names: Vec<&str> = [AGENT, RUNNER, SERVICE, SOCKET]
        .into_iter()
        .filter(|n| dir.join(n).exists())
        .collect();
    if names.is_empty() {
        return Ok(false);
    }
    systemctl(&[&["disable", "--now"], names.as_slice()].concat())?;
    let files: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
    for f in &files {
        match fs::remove_file(f) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(Check::fail(
                    "service",
                    format!("cannot remove {}: {e}", f.display()),
                    "",
                ))
            }
        }
    }
    systemctl(&["daemon-reload"])?;
    Ok(true)
}

/// Whether systemd keeps this user's services running after they log out.
pub(crate) fn lingers(env: &Env) -> Option<bool> {
    let user = env.get("USER").filter(|u| !u.is_empty())?;
    Some(Path::new("/var/lib/systemd/linger").join(user).exists())
}

/// `systemctl --user args`, failing with what it said.
fn systemctl(args: &[&str]) -> Result<(), Check> {
    let out = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            Check::fail(
                "service",
                format!("cannot run systemctl: {e}"),
                "moor up needs a systemd user manager",
            )
        })?;
    if out.status.success() {
        return Ok(());
    }
    Err(Check::fail(
        "service",
        format!(
            "systemctl --user {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        "check `systemctl --user status` works in this shell",
    ))
}
