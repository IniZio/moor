//! The controller as a systemd user service: a socket unit that holds the controller socket
//! across restarts, and the service it starts. Both belong to the person, so no root.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::files::{write, Layout};
use crate::doctor::Check;
use crate::env::Env;

/// The service unit's name.
pub(crate) const SERVICE: &str = "moor.service";
/// The socket unit's name.
pub(crate) const SOCKET: &str = "moor.socket";

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
    /// The unit for `layout`, run by this executable with this `PATH`.
    pub(crate) fn new(env: &Env, layout: &Layout) -> Result<Self, Check> {
        let exe = std::env::current_exe()
            .map_err(|e| Check::fail("service", format!("cannot find moor itself: {e}"), ""))?;
        let mut vars = vec![(
            "MOOR_CONFIG".to_owned(),
            layout.user_config.display().to_string(),
        )];
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

/// Stops and disables both units and removes their files; whether any was there.
pub(crate) fn remove(dir: &Path) -> Result<bool, Check> {
    let files = [dir.join(SERVICE), dir.join(SOCKET)];
    if !files.iter().any(|f| f.exists()) {
        return Ok(false);
    }
    systemctl(&["disable", "--now", SERVICE, SOCKET])?;
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
