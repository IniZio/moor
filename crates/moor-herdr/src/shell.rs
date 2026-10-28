//! herdr's default shell: a new pane in a Box's workspace opens a login shell in that Box;
//! every other pane gets the user's own shell, as if moor were not there.

use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;

use moor_client::Client;
use moor_core::request::AttachReq;
use serde_json::json;

use crate::boxes::{open_box, Phase, Shown, BOX_MARK};
use crate::card::Ended;
use crate::error::Error;
use crate::herdr::Space;
use crate::relay::{feed, pump, size, Raw};

/// Set on the pane sync creates for a Box's agent, so that pane stays a host shell for sync's
/// attach command to run in.
pub const AGENT_PANE: (&str, &str) = ("MOOR_HERDR_PANE", "agent");

/// The plugin env file's name for the shell a pane runs when it is not a Box shell.
pub const HOST_SHELL_ENV: &str = "MOOR_HERDR_HOST_SHELL";

/// Where a new pane's shell runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// In the open, up Box of this name.
    Box(String),
    /// On the host, as a plain workspace's pane does.
    Host,
    /// On the host although the workspace is a Box's, for the reason the user is told.
    Fallback(String),
}

/// What the pane knows before asking anyone.
#[derive(Debug, Clone, Copy)]
pub struct Pane<'a> {
    /// `HERDR_WORKSPACE_ID`, when herdr manages the pane.
    pub workspace: Option<&'a str>,
    /// Whether sync made the pane to run the agent's attach.
    pub agent: bool,
    /// Whether the controller socket is there to dial.
    pub socket: bool,
}

/// Decides where `pane`'s shell runs: one herdr call for the workspace label, then one
/// `show` of the Box it names. A Box workspace with no controller still says why.
pub fn decide(
    pane: Pane<'_>,
    spaces: impl FnOnce() -> Result<Vec<Space>, Error>,
    show: impl FnOnce(&str) -> Result<Option<Shown>, Error>,
) -> Target {
    let Some(id) = pane.workspace.filter(|_| !pane.agent) else {
        return Target::Host;
    };
    let Some(label) = spaces()
        .ok()
        .and_then(|all| all.into_iter().find(|s| s.id == id))
        .map(|s| s.label)
        .filter(|l| l.starts_with(BOX_MARK))
    else {
        return Target::Host;
    };
    let name = label
        .rsplit(['/', ' '])
        .next()
        .unwrap_or_default()
        .to_owned();
    if !pane.socket {
        return Target::Fallback(format!("no moor controller to reach Box {name}"));
    }
    match show(&name) {
        Ok(Some(b)) => on(&b),
        Ok(None) => Target::Fallback(format!("Box {name} is closed")),
        Err(e) => Target::Fallback(format!("Box {name} could not be reached: {e}")),
    }
}

/// Where a shell in Box `b` runs, by its phase.
fn on(b: &Shown) -> Target {
    let why = match b.phase {
        Phase::Live(_) => return Target::Box(b.name.clone()),
        Phase::Asleep => "is asleep",
        Phase::Starting => "is still being set up",
        Phase::WaitingForMachine => "waits for a slot on its machine",
        Phase::Closed => "is closed",
    };
    Target::Fallback(format!("Box {} {why}", b.name))
}

/// The user's own shell: the one setup recorded, else `$SHELL` unless that is this wrapper
/// (herdr's login mode sets it so), else `/bin/sh`.
#[must_use]
pub fn host_shell(recorded: Option<String>, shell: Option<String>) -> String {
    let ours = |s: &String| Path::new(s).file_name().is_some_and(|n| n == "shell.sh");
    recorded
        .filter(|s| !s.trim().is_empty())
        .or_else(|| shell.filter(|s| !s.trim().is_empty() && !ours(s)))
        .unwrap_or_else(|| "/bin/sh".to_owned())
}

/// Replaces this process with the user's own shell, a login shell when `login`.
fn exec_host(login: bool) -> Error {
    let shell = host_shell(
        std::env::var(HOST_SHELL_ENV).ok(),
        std::env::var("SHELL").ok(),
    );
    let mut cmd = Command::new(&shell);
    if login {
        let base = Path::new(&shell).file_name().unwrap_or_default();
        cmd.arg0(format!("-{}", base.to_string_lossy()));
    }
    Error::Io(cmd.exec())
}

/// Relays a login shell in Box `name` until it ends.
fn relay_shell(client: &Client, name: &str) -> Result<Ended, Error> {
    let raw = Raw::enter()?;
    let (cols, rows) = size();
    let req: AttachReq =
        serde_json::from_value(json!({"box": name, "cols": cols, "rows": rows, "shell": true}))?;
    let (screen, keys) = client.attach(req)?;
    let (tx, rx) = mpsc::channel();
    feed(&tx)?;
    let ended = pump(screen, keys, (&tx, &rx), io::stdout());
    drop(raw);
    Ok(ended)
}

/// Runs a new pane's shell. Returns only when a Box shell ended, or with the error that kept
/// the user's own shell from starting.
pub fn run(socket: Option<&Path>, login: bool) -> Result<(), Error> {
    let workspace = std::env::var("HERDR_WORKSPACE_ID").ok();
    let pane = Pane {
        workspace: workspace.as_deref(),
        agent: std::env::var(AGENT_PANE.0).is_ok_and(|v| v == AGENT_PANE.1),
        socket: socket.is_some_and(Path::exists),
    };
    let client = socket.map(Client::new);
    let target = decide(
        pane,
        || crate::herdr::Herdr::spaces(&mut crate::herdr::Cli::from_env()),
        |name| client.as_ref().map_or(Ok(None), |c| open_box(c, name)),
    );
    let why = match (&target, &client) {
        (Target::Box(name), Some(c)) => match relay_shell(c, name) {
            Ok(Ended::Refused(why)) => format!("Box {name}: {why}"),
            Ok(_) => return Ok(()),
            Err(e) => format!("Box {name}: {e}"),
        },
        (Target::Fallback(why), _) => why.clone(),
        _ => return Err(exec_host(login)),
    };
    let _ = writeln!(io::stderr(), "moor: {why}; this is a local shell");
    Err(exec_host(login))
}

#[cfg(test)]
mod tests;
