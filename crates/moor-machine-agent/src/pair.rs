//! Pairing this machine with its owner by a code they confirm in any browser, so a host with
//! no browser of its own pairs too. The gate's side follows RFC 8628's device authorization.

use std::path::Path;
use std::time::Duration;

use moor_client::oidc::Http;
use serde::Deserialize;
use serde_json::json;

use crate::{key, private_dir, takes_no_machines, write_private, State, KEY};

/// What a pairing asks the server for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// The machine's name, unique among its owner's machines.
    pub name: String,
    /// The machine's labels.
    pub labels: Vec<String>,
    /// How many Boxes may hold the machine at once.
    pub max_boxes: u16,
    /// The crabbox provider the machine offers Boxes on; none for a machine that runs `exec`
    /// scripts instead.
    pub provider: Option<String>,
}

/// The gate's answer to a pairing.
#[derive(Deserialize)]
struct Started {
    /// The secret to poll with.
    device_code: String,
    /// The code the person checks.
    user_code: String,
    /// Where the person confirms it, the code included.
    verification_uri_complete: String,
    /// Seconds the code lasts.
    expires_in: u64,
    /// Seconds between polls.
    interval: u64,
}

/// What the gate answers a confirmed poll with.
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

/// Asks `server` whether it pairs machines, before anything else is done for it.
///
/// # Errors
/// The server routes no pairing, refuses it, or cannot be reached.
pub fn check_server(server: &str) -> Result<(), String> {
    let url = format!("{server}/machines/pair/poll");
    match Http::new().post_json_found::<serde_json::Value>(&url, &json!({"device_code": ""})) {
        Ok(None) => Err(takes_no_machines(server)),
        Err(e) if !e.ends_with(": expired_token") => Err(e),
        _ => Ok(()),
    }
}

/// Pairs this machine with `server` as `ask`, keeping its key in `dir`: shows the person where
/// to confirm through `say`, polls until they do, and saves the controller's answer.
///
/// # Errors
/// The key or state cannot be kept, the server refuses, or the code expires unconfirmed.
pub fn pair(
    server: &str,
    ask: &Ask,
    dir: &Path,
    say: &mut dyn FnMut(&str),
) -> Result<State, String> {
    private_dir(dir).map_err(|e| format!("cannot prepare {}: {e}", dir.display()))?;
    let pubkey = key(&dir.join(KEY))?
        .public_key()
        .to_openssh()
        .map_err(|e| format!("cannot encode the public key: {e}"))?;
    let http = Http::new();
    let body = json!({"pubkey": pubkey, "name": ask.name, "labels": ask.labels,
        "slots": ask.max_boxes, "pool": ask.provider});
    let started: Started = http
        .post_json_found(&format!("{server}/machines/pair"), &body)?
        .ok_or_else(|| takes_no_machines(server))?;
    say(&format!(
        "to add {} to your machines, open {} on any device and check it shows {} ({} minutes left)",
        ask.name,
        started.verification_uri_complete,
        started.user_code,
        started.expires_in / 60
    ));
    let poll = format!("{server}/machines/pair/poll");
    let wait = Duration::from_secs(started.interval.max(1));
    let got =
        loop {
            std::thread::sleep(wait);
            match http.post_json(&poll, &json!({"device_code": started.device_code})) {
                Ok(got) => break got,
                Err(e) if e.ends_with(": authorization_pending") => {}
                Err(e) if e.ends_with(": expired_token") => return Err(
                    "the code expired before anyone confirmed it; run moor-machine-agent up again"
                        .to_owned(),
                ),
                Err(e) => return Err(e),
            }
        };
    let Enrolled {
        machine_id,
        name,
        owner,
        capacity,
    } = got;
    let state = State {
        server: server.to_owned(),
        machine_id,
        owner,
        name,
        slots: capacity,
    };
    let text = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
    write_private(&dir.join("state.json"), text.as_bytes())
        .map_err(|e| format!("cannot save the state: {e}"))?;
    Ok(state)
}
