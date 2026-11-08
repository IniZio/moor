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

pub use crate::pool::Pool;
pub use crate::release::{update, Updated};
pub use crate::serve::Machine;

mod busy;
mod files;
mod held;
mod listen;
pub mod pool;
mod release;
mod serve;
pub mod settings;
mod status;
mod sweep;
mod upgrade;

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
/// The longest wait between dials after a relink, while the next runner comes up.
const RELINK_MOST_WAIT: Duration = Duration::from_secs(2);
/// How long after a relink the waits stay that short.
const RELINK_WINDOW: Duration = Duration::from_secs(120);
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
    let got: Enrolled = Http::new()
        .post_json_found(&format!("{}/machines/enroll", signed.server), &body)?
        .ok_or_else(|| takes_no_machines(&signed.server))?;
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
    run_with(dir, machine, false, log).await
}

/// As [`run`]; with `auto_update`, the agent moves to the release of the server it links to.
///
/// It installs that release, then starts it in this process's place with the same arguments
/// once no stream or boot is going on. Linked, it keeps serving meanwhile.
///
/// # Errors
/// As [`run`], and the update or the restart fails.
pub async fn run_with(
    dir: &Path,
    machine: Machine,
    auto_update: bool,
    log: &(dyn Fn(&str) + Sync),
) -> Result<(), String> {
    let machine = Arc::new(machine);
    let path = dir.join("state.json");
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}; enroll first", path.display()))?;
    let state: State = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not the agent's state: {e}", path.display()))?;
    let key = read_key(&dir.join(KEY))?
        .ok_or_else(|| format!("{} has no key; enroll first", dir.display()))?;
    let (tell, offers) = tokio::sync::watch::channel(String::new());
    tokio::select! {
        done = keep_linked(&state, &key, &machine, auto_update, log, &tell) => done,
        never = upgrade::follow(offers, auto_update, log) => match never {},
    }
}

/// Keeps `machine` linked to `state`'s server, telling `tell` the server's release on each link.
async fn keep_linked(
    state: &State,
    key: &PrivateKey,
    machine: &Arc<Machine>,
    auto_update: bool,
    log: &(dyn Fn(&str) + Sync),
    tell: &tokio::sync::watch::Sender<String>,
) -> Result<(), String> {
    let boxes = listen::Boxes::new(state.slots);
    let recent = Arc::new(status::Recent::default());
    let mut wait = FIRST_WAIT;
    // The link a relink left open; while it is, the new runner is coming up, so no backoff.
    let mut old: Option<tokio::task::JoinHandle<String>> = None;
    let mut relinked: Option<tokio::time::Instant> = None;
    loop {
        let (why, handing_over) = match connect(state, key, machine).await {
            Ok((carried, theirs)) => {
                log(&format!("linked to {}", state.server));
                tell.send_replace(theirs);
                wait = FIRST_WAIT;
                relinked = None;
                match held::hold(carried, machine, &boxes, &recent).await {
                    held::Ended::Closed(why) => (why, false),
                    held::Ended::Relinked(link) => {
                        old = Some(link);
                        relinked = Some(tokio::time::Instant::now());
                        log("the server asked for a new link; dialing again now");
                        continue;
                    }
                }
            }
            Err(Turned::Update(version)) => {
                return Err(upgrade::required(&version, auto_update, log).await);
            }
            Err(Turned::Relink) => {
                relinked.get_or_insert_with(tokio::time::Instant::now);
                ("the server is handing over".to_owned(), true)
            }
            Err(Turned::NoMachines) => return Err(takes_no_machines(&state.server)),
            Err(Turned::Failed(why)) => (why, false),
        };
        if handing_over || old.as_ref().is_some_and(|l| !l.is_finished()) {
            log(&format!(
                "{why}; dialing again in {}s",
                FIRST_WAIT.as_secs()
            ));
            tokio::time::sleep(FIRST_WAIT).await;
            continue;
        }
        old = None;
        let pause = match relinked {
            Some(at) if at.elapsed() < RELINK_WINDOW => wait.min(RELINK_MOST_WAIT),
            _ => wait,
        };
        log(&format!("{why}; dialing again in {}s", pause.as_secs()));
        tokio::time::sleep(pause).await;
        wait = (wait * 2).min(MOST_WAIT);
    }
}

/// Why a dial did not end in a link.
enum Turned {
    /// The server needs this release.
    Update(String),
    /// The server is stopping and hands its links to another; dial again shortly.
    Relink,
    /// The server routes no link path, so no machine can ever link to it.
    NoMachines,
    /// Anything else; dialing again may work.
    Failed(String),
}

/// What to tell the person when `server` answers 404 where machines enroll or link.
fn takes_no_machines(server: &str) -> String {
    let host = server
        .trim_end_matches('/')
        .split_once("://")
        .map_or(server, |(_, rest)| rest);
    format!("the server at {host} takes no machines; its operator must enable the machine runner")
}

/// Dials `state`'s server and proves the machine with `key`; the link, once it is up, and the
/// server's release.
async fn connect(
    state: &State,
    key: &PrivateKey,
    machine: &Machine,
) -> Result<(link::Carried, String), Turned> {
    let mut ws = link::dial_found(&state.server)
        .await
        .map_err(Turned::Failed)?
        .ok_or(Turned::NoMachines)?;
    let hello = Frame::Hello {
        machine_id: state.machine_id,
        version: VERSION.to_owned(),
        offer: pool::offer(machine).await.map(Box::new),
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
        Frame::Welcome { version } => Ok((link::carry(ws, link::KEEPALIVE), version)),
        other => Err(turned(other)),
    }
}

/// What the server's `frame` in place of the next handshake step means.
fn turned(frame: Frame) -> Turned {
    match frame {
        Frame::UpdateRequired { version } => Turned::Update(version),
        Frame::Relink {} => Turned::Relink,
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
pub(crate) fn unix_now() -> i64 {
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
