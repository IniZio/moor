//! The machine agent: runs on a person's own computer, enrolls it with their sign-in, and keeps
//! it linked to the server, running the execs, hooks and relays the server asks of its Boxes.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moor_client::link::{self, Frame};
use moor_client::oidc::{self, Http, Ui};
use serde::{Deserialize, Serialize};
use serde_json::json;
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey};
use tokio::sync::mpsc;

pub use crate::pool::Pool;
pub use crate::serve::Machine;

mod files;
mod listen;
pub mod pool;
mod serve;

/// What `enroll` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enroll {
    /// The moor server.
    pub server: String,
    /// The machine's name, unique among its owner's machines.
    pub name: String,
    /// The machine's labels.
    pub labels: Vec<String>,
    /// How many Boxes may hold the machine at once.
    pub slots: u16,
    /// The crabbox provider the machine offers Boxes on, when it hosts a Box pool.
    pub pool: Option<String>,
    /// Replace the machine's key instead of keeping it.
    pub new_key: bool,
}

/// What the agent keeps after enrolling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// The moor server, without a trailing slash.
    pub server: String,
    /// The id the controller gave the machine.
    pub machine_id: i64,
    /// The owner's verified email.
    pub owner: String,
    /// The machine's name.
    pub name: String,
    /// How many Boxes may hold the machine at once.
    pub slots: u16,
}

/// What the gate answers an enrollment with.
#[derive(Deserialize)]
struct Enrolled {
    /// The machine's id.
    machine_id: i64,
    /// The machine's name.
    name: String,
    /// The owner's verified email.
    owner: String,
    /// The slots the controller recorded.
    capacity: u16,
}

/// The private key file, OpenSSH format.
const KEY: &str = "id_ed25519";
/// The agent's release, which the server checks.
const VERSION: &str = match option_env!("MOOR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
/// The first wait before dialing again; it doubles up to [`MOST_WAIT`] while dials fail.
const FIRST_WAIT: Duration = Duration::from_secs(1);
/// The longest wait between dials.
const MOST_WAIT: Duration = Duration::from_secs(60);
/// How long each handshake frame may take to arrive.
const ANSWER: Duration = Duration::from_secs(10);

/// Where the agent keeps its key and state: `$XDG_STATE_HOME/moor-machine-agent`.
#[must_use]
pub fn state_dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map_or_else(
        || Path::new(&std::env::var_os("HOME").unwrap_or_default()).join(".local/state"),
        PathBuf::from,
    );
    state.join("moor-machine-agent")
}

/// Enrolls this machine: keeps or makes its key in `dir`, signs its owner in (the browser
/// returns to loopback `port`; 0 takes any free one), and saves the controller's answer.
///
/// # Errors
/// The key or state cannot be read or written, the sign-in fails, or
/// the server refuses the enrollment.
pub fn enroll(req: &Enroll, dir: &Path, port: u16, ui: &mut dyn Ui) -> Result<State, String> {
    private_dir(dir).map_err(|e| format!("cannot prepare {}: {e}", dir.display()))?;
    let key = key(&dir.join(KEY), req.new_key)?;
    let pubkey = key
        .public_key()
        .to_openssh()
        .map_err(|e| format!("cannot encode the public key: {e}"))?;
    let signed = oidc::sign_in(&req.server, port, ui)?;
    let body = json!({"access_token": signed.access_token, "pubkey": pubkey, "name": req.name,
        "labels": req.labels, "slots": req.slots, "pool": req.pool});
    let got: Enrolled =
        Http::new().post_json(&format!("{}/machines/enroll", signed.server), &body)?;
    let state = State {
        server: signed.server,
        machine_id: got.machine_id,
        owner: got.owner,
        name: got.name,
        slots: got.capacity,
    };
    let text = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
    write_private(&dir.join("state.json"), text.as_bytes())
        .map_err(|e| format!("cannot save the state: {e}"))?;
    Ok(state)
}

/// Keeps this machine linked to its server with the key and state enrollment left in `dir`,
/// dialing again after every drop, and telling `log` each change.
///
/// # Errors
/// Enrollment left nothing usable in `dir`, or the server says this agent must be updated.
pub async fn run(dir: &Path, machine: Machine, log: &(dyn Fn(&str) + Sync)) -> Result<(), String> {
    let machine = Arc::new(machine);
    let path = dir.join("state.json");
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}; enroll first", path.display()))?;
    let state: State = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not the agent's state: {e}", path.display()))?;
    let key = read_key(&dir.join(KEY))?
        .ok_or_else(|| format!("{} has no key; enroll first", dir.display()))?;
    let boxes = listen::Boxes::new(state.slots);
    let mut wait = FIRST_WAIT;
    loop {
        match connect(&state, &key).await {
            Ok(socket) => {
                log(&format!("linked to {}", state.server));
                wait = FIRST_WAIT;
                let (open, asks) = mpsc::channel(16);
                let streams = listen::Link {
                    open,
                    boxes: Arc::clone(&boxes),
                };
                let why = link::drive(socket, link::Mode::Client, asks, |ch| {
                    let (machine, streams) = (Arc::clone(&machine), streams.clone());
                    tokio::spawn(async move { serve::serve(&machine, &streams, ch).await });
                })
                .await;
                log(&format!("{why}; dialing again in {}s", wait.as_secs()));
            }
            Err(Turned::Update(version)) => {
                return Err(format!(
                    "the server needs this agent updated from {VERSION} to {version}"
                ));
            }
            Err(Turned::Failed(why)) => {
                log(&format!("{why}; dialing again in {}s", wait.as_secs()));
            }
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(MOST_WAIT);
    }
}

/// Why a dial did not end in a link.
enum Turned {
    /// The server needs this release.
    Update(String),
    /// Anything else; dialing again may work.
    Failed(String),
}

/// Dials `state`'s server and proves the machine with `key`; the socket yamux runs on.
async fn connect(state: &State, key: &PrivateKey) -> Result<link::Socket, Turned> {
    let mut ws = link::dial(&state.server).await.map_err(Turned::Failed)?;
    let hello = Frame::Hello {
        machine_id: state.machine_id,
        protocol: link::PROTOCOL,
        version: VERSION.to_owned(),
    };
    link::send(&mut ws, &hello).await.map_err(Turned::Failed)?;
    let nonce = match link::recv(&mut ws, ANSWER).await.map_err(Turned::Failed)? {
        Frame::Challenge { nonce } => nonce,
        other => return Err(turned(other)),
    };
    let time = unix_now();
    let signature = key
        .sign(
            link::NAMESPACE,
            HashAlg::Sha512,
            &link::signed(&nonce, state.machine_id, time),
        )
        .and_then(|sig| sig.to_pem(LineEnding::LF))
        .map_err(|e| Turned::Failed(format!("cannot sign the challenge: {e}")))?;
    link::send(&mut ws, &Frame::Proof { time, signature })
        .await
        .map_err(Turned::Failed)?;
    match link::recv(&mut ws, ANSWER).await.map_err(Turned::Failed)? {
        Frame::Welcome { .. } => Ok(link::carry(ws, link::KEEPALIVE)),
        other => Err(turned(other)),
    }
}

/// What the server's `frame` in place of the next handshake step means.
fn turned(frame: Frame) -> Turned {
    match frame {
        Frame::UpdateRequired { version } => Turned::Update(version),
        Frame::Refused { reason } => {
            Turned::Failed(format!("the server refused the link: {reason}"))
        }
        other => Turned::Failed(format!("the server answered out of turn: {other:?}")),
    }
}

/// Unix seconds now, which the proof carries.
#[expect(
    clippy::disallowed_methods,
    reason = "the proof carries wall-clock time"
)]
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The ed25519 key at `path`, or `None` when there is none.
fn read_key(path: &Path) -> Result<Option<PrivateKey>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let key = PrivateKey::from_openssh(text)
        .map_err(|e| format!("{} is not an OpenSSH key: {e}", path.display()))?;
    if key.algorithm() == Algorithm::Ed25519 {
        Ok(Some(key))
    } else {
        Err(format!("{} is not an ed25519 key", path.display()))
    }
}

/// Creates `dir` if needed and makes it the owner's alone.
fn private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// The ed25519 key at `path`, made there when absent or when `fresh`.
fn key(path: &Path, fresh: bool) -> Result<PrivateKey, String> {
    if !fresh {
        if let Some(key) = read_key(path)? {
            return Ok(key);
        }
    }
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
        .map_err(|e| format!("cannot make a key: {e}"))?;
    let text = key
        .to_openssh(LineEnding::LF)
        .map_err(|e| format!("cannot encode the key: {e}"))?;
    write_private(path, text.as_bytes())
        .map_err(|e| format!("cannot save {}: {e}", path.display()))?;
    Ok(key)
}

/// Replaces `path` with `bytes`, readable by the owner alone at every moment.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
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
