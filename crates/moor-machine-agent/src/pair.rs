//! Pairing this machine with its owner by a code they confirm in any browser, so a host with
//! no browser of its own pairs too. The gate's side follows RFC 8628's device authorization.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use moor_client::oidc::Http;
use serde::Deserialize;
use serde_json::json;

use crate::{key, private_dir, takes_no_machines, write_private, State, KEY};

/// The slots a pairing asks for. A machine that makes Boxes has no cap of its own, since a node's
/// cap is its setting in the controller, so this sizes only the slots a tool machine's Boxes take.
const SLOTS: u16 = 4;

/// What a pairing asks the server for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// The machine's name, unique among its owner's machines.
    pub name: String,
    /// The machine's labels.
    pub labels: Vec<String>,
    /// What the machine hosts: `boxes` when it makes Boxes itself; none for a machine that runs
    /// `exec` scripts instead.
    pub pool: Option<String>,
}

/// The one-time token a portal minted, which enrolls this machine with no confirm page. It goes
/// only into the pairing request: never into state, the service, or output.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// A token as the person pasted it.
    #[must_use]
    pub const fn new(token: String) -> Self {
        Self(token)
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

/// The gate's answer to a pairing: the enrollment when a token paired the machine, else a code
/// for its owner to confirm.
#[derive(Deserialize)]
#[serde(untagged)]
enum Answer {
    /// A token enrolled the machine.
    Enrolled {
        /// The controller's enrollment.
        enrolled: Enrolled,
    },
    /// A code to confirm.
    Started(Started),
}

/// The gate's answer to a pairing with no token.
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

/// Pairs this machine with `server` as `ask`, keeping its key in `dir`.
///
/// With `token` the server enrolls it at once. Without, it shows the person where to confirm
/// through `say` and polls until they do. Either way it saves the controller's answer.
///
/// # Errors
/// The key or state cannot be kept, the server refuses, or the code expires unconfirmed.
pub fn pair(
    server: &str,
    ask: &Ask,
    token: Option<&Token>,
    dir: &Path,
    say: &mut dyn FnMut(&str),
) -> Result<State, String> {
    private_dir(dir).map_err(|e| format!("cannot prepare {}: {e}", dir.display()))?;
    let pubkey = key(&dir.join(KEY))?
        .public_key()
        .to_openssh()
        .map_err(|e| format!("cannot encode the public key: {e}"))?;
    let http = Http::new();
    let mut body = json!({"pubkey": pubkey, "name": ask.name, "labels": ask.labels,
        "slots": SLOTS, "pool": ask.pool});
    if let (Some(Token(token)), Some(fields)) = (token, body.as_object_mut()) {
        fields.insert("token".to_owned(), json!(token));
    }
    let answer: Answer = http
        .post_json_found(&format!("{server}/machines/pair"), &body)?
        .ok_or_else(|| takes_no_machines(server))?;
    let got = match answer {
        Answer::Enrolled { enrolled } => {
            say(&format!("added {} to your machines", enrolled.name));
            enrolled
        }
        Answer::Started(started) => confirmed(&http, server, ask, &started, say)?,
    };
    keep(got, server, ask, dir)
}

/// Shows where `started`'s code is confirmed and polls until it is; the enrollment it gets.
fn confirmed(
    http: &Http,
    server: &str,
    ask: &Ask,
    started: &Started,
    say: &mut dyn FnMut(&str),
) -> Result<Enrolled, String> {
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
    Ok(got)
}

/// Saves the machine's state under `dir`, as `got` enrolled it.
fn keep(got: Enrolled, server: &str, ask: &Ask, dir: &Path) -> Result<State, String> {
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
        pool: ask.pool.clone(),
    };
    let text = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
    write_private(&dir.join("state.json"), text.as_bytes())
        .map_err(|e| format!("cannot save the state: {e}"))?;
    Ok(state)
}
