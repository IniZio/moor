//! `moor node add`: enroll a machine agent on this host as a shared node, with no sign-in.
//!
//! The operator runs it on the controller's host. It makes the agent's key and, once the
//! controller has recorded the machine, the state the agent links with, laid out where an agent
//! run as `HOME=<agent home>` reads them. The controller holds the machine with no owner, which
//! makes its node shared.

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::Write as _;
use std::net::IpAddr;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use moor_core::request::NodeAddReq;
use moor_core::view::node::NodeAddValue;
use url::{Host, Url};

/// What `moor node add` was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Add {
    /// The node's name, which is its machine's.
    pub name: String,
    /// The `HOME` the agent will run under.
    pub agent_home: PathBuf,
    /// Where the runner takes agent links, with a scheme; plain `http` only on loopback.
    pub runner: String,
    /// The machine's labels.
    pub labels: Vec<String>,
    /// How many Boxes may hold the machine at once.
    pub max_boxes: u16,
}

/// The agent's key file, as the agent names it.
const KEY: &str = "id_ed25519";
/// The agent's state file, as the agent reads it.
const STATE: &str = "state.json";
/// What the agent's state names as its owner; a shared node has none.
const OWNER: &str = "operator";

/// Where an agent run as `HOME=<home>` keeps its key and state.
#[must_use]
pub fn agent_dir(home: &Path) -> PathBuf {
    home.join(".local/state/moor-machine-agent")
}

/// The runner address `given` names as the agent dials it, `http://host:port` or `https://...`.
///
/// A bare `host:port` is `http`. Plain `http` is only for the loopback of this host, so a link
/// that leaves it is always TLS.
///
/// # Errors
/// `given` is not an address, or it asks plain `http` away from loopback.
pub fn runner_url(given: &str) -> Result<String, String> {
    let with_scheme = if given.contains("://") {
        given.to_owned()
    } else {
        format!("http://{given}")
    };
    let url = Url::parse(&with_scheme).map_err(|e| format!("--runner {given}: {e}"))?;
    if url.port().is_none() && url.scheme() == "http" {
        return Err(format!("--runner {given} needs a port"));
    }
    let loopback = match url.host() {
        Some(Host::Ipv4(a)) => IpAddr::V4(a).is_loopback(),
        Some(Host::Ipv6(a)) => IpAddr::V6(a).is_loopback(),
        Some(Host::Domain(d)) => d == "localhost",
        None => return Err(format!("--runner {given} names no host")),
    };
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        "http" => {
            return Err(format!(
                "--runner {given} is plain http away from loopback; use https, or 127.0.0.1"
            ))
        }
        other => return Err(format!("--runner {given}: {other} is not http or https")),
    }
    Ok(with_scheme.trim_end_matches('/').to_owned())
}

/// Makes the agent's directory and key under `add.agent_home`, keeping a key already there, and
/// the request that enrolls its public half.
///
/// # Errors
/// The runner address is refused, or the directory or key cannot be made.
pub fn prepare(add: &Add) -> Result<NodeAddReq, String> {
    runner_url(&add.runner)?;
    let dir = agent_dir(&add.agent_home);
    private_dirs(&add.agent_home, &dir)?;
    let key = dir.join(KEY);
    if !key.exists() {
        make_key(&key, &add.name)?;
    }
    let public = fs::read_to_string(key.with_extension("pub"))
        .map_err(|e| format!("cannot read {}: {e}", key.with_extension("pub").display()))?;
    let mut fields = public.split_whitespace();
    let pubkey = match (fields.next(), fields.next()) {
        (Some(kind), Some(blob)) => format!("{kind} {blob}"),
        _ => return Err(format!("{} holds no public key", key.display())),
    };
    Ok(NodeAddReq {
        name: add.name.clone(),
        pubkey,
        labels: add.labels.clone(),
        capacity: add.max_boxes,
    })
}

/// Writes the state the agent links with, now that the controller holds machine `added`.
///
/// # Errors
/// The runner address is refused or the state cannot be written.
pub fn finish(add: &Add, added: &NodeAddValue) -> Result<(), String> {
    let state = serde_json::json!({
        "server": runner_url(&add.runner)?,
        "machine_id": added.machine_id,
        "owner": OWNER,
        "name": added.name,
        "slots": add.max_boxes,
    });
    let text = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
    let path = agent_dir(&add.agent_home).join(STATE);
    write_private(&path, text.as_bytes())
        .map_err(|e| format!("cannot save {}: {e}", path.display()))
}

/// The lines that tell the operator how to run the agent.
#[must_use]
pub fn run_lines(add: &Add) -> String {
    format!(
        "run:\n  HOME={} moor-machine-agent run\n",
        add.agent_home.display()
    )
}

/// `home` and `dir` under it, each for the owner alone.
fn private_dirs(home: &Path, dir: &Path) -> Result<(), String> {
    for path in [home, dir] {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .and_then(|()| fs::set_permissions(path, Permissions::from_mode(0o700)))
            .map_err(|e| format!("cannot prepare {}: {e}", path.display()))?;
    }
    Ok(())
}

/// An ed25519 key at `key`, with its public half beside it.
fn make_key(key: &Path, name: &str) -> Result<(), String> {
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"])
        .arg(key)
        .stdin(Stdio::null())
        .status();
    if made.is_ok_and(|s| s.success()) {
        Ok(())
    } else {
        Err(format!("cannot make the agent's key {}", key.display()))
    }
}

/// Replaces `path` with `bytes`, readable by the owner alone at every moment.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests;
