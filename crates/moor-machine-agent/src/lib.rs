//! The machine agent: runs on a person's own computer and enrolls it with their sign-in, so
//! Boxes the controller starts can later claim it.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use moor_client::oidc::{self, Http, Ui};
use serde::{Deserialize, Serialize};
use serde_json::json;
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};

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
/// The key or state cannot be read or written, the sign-in fails or grants no id token, or
/// the server refuses the enrollment.
pub fn enroll(req: &Enroll, dir: &Path, port: u16, ui: &mut dyn Ui) -> Result<State, String> {
    private_dir(dir).map_err(|e| format!("cannot prepare {}: {e}", dir.display()))?;
    let key = key(&dir.join(KEY), req.new_key)?;
    let pubkey = key
        .public_key()
        .to_openssh()
        .map_err(|e| format!("cannot encode the public key: {e}"))?;
    let signed = oidc::sign_in(&req.server, port, ui)?;
    let id_token = signed
        .id_token
        .ok_or("the sign-in returned no id token; the openid scope is needed")?;
    let body = json!({"id_token": id_token, "pubkey": pubkey, "name": req.name,
        "labels": req.labels, "slots": req.slots});
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

/// Creates `dir` if needed and makes it the owner's alone.
fn private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// The ed25519 key at `path`, made there when absent or when `fresh`.
fn key(path: &Path, fresh: bool) -> Result<PrivateKey, String> {
    if !fresh {
        match fs::read_to_string(path) {
            Ok(text) => {
                let key = PrivateKey::from_openssh(text)
                    .map_err(|e| format!("{} is not an OpenSSH key: {e}", path.display()))?;
                return if key.algorithm() == Algorithm::Ed25519 {
                    Ok(key)
                } else {
                    Err(format!("{} is not an ed25519 key", path.display()))
                };
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
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
