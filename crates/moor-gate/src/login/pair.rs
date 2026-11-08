//! Pairing a machine by code, in the shape of OAuth's device authorization grant (RFC 8628):
//! the agent posts its key to `/machines/pair` and gets a short code, its owner confirms the code
//! at `/pair` behind browser sign-in, and the agent's poll at `/machines/pair/poll` gets the
//! enrollment the confirm made. A code waits in this gate's memory, so pairing needs one gate.

use std::collections::HashMap;
use std::io::Read;
use std::net::TcpStream;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::OsRng;
use oauth2::CsrfToken;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{Login, RANDOM_BYTES};
use crate::http::respond;

pub(crate) mod page;
#[cfg(test)]
mod tests;

/// The confirm page's path.
pub(crate) const PATH: &str = "/pair";

/// How long a code can be confirmed and its answer collected.
const TTL: Duration = Duration::from_mins(10);

/// Seconds the agent waits between polls.
const INTERVAL_SECS: u64 = 5;

/// The most codes waiting at once; more are refused until some are used or expire.
const MOST_WAITING: usize = 256;

/// Letters a code is made of: no vowels, so no word, and none that reads as a digit.
const ALPHABET: &[u8; 20] = b"BCDFGHJKLMNPQRSTVWXZ";

/// Largest JSON body the agent routes read.
const BODY_MAX: usize = 16 * 1024;

/// What the agent asks to be enrolled as.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ask {
    /// The agent's ed25519 public key as an OpenSSH line.
    pubkey: String,
    /// The machine's name.
    name: String,
    /// The machine's labels.
    #[serde(default)]
    labels: Vec<String>,
    /// How many Boxes may hold the machine at once.
    slots: u16,
    /// The crabbox provider the machine offers Boxes on, when it hosts a Box pool.
    #[serde(default)]
    pool: Option<String>,
}

/// Where a code is.
enum Stage {
    /// Shown to the agent; nobody has confirmed it.
    Waiting,
    /// A confirm is enrolling the machine now.
    Enrolling,
    /// Enrolled; the controller's answer waits for the agent's next poll.
    Enrolled(Value),
}

/// One code waiting to be confirmed or collected.
struct Pairing {
    /// The secret the agent polls with.
    device_code: String,
    /// What the agent asked for.
    ask: Ask,
    /// When the code stops working.
    expires: SystemTime,
    /// How far it got.
    stage: Stage,
}

/// What a poll finds.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Polled {
    /// Not confirmed yet.
    Pending,
    /// Unknown, expired or already collected.
    Expired,
    /// The controller's enrollment, handed out this once.
    Enrolled(Value),
}

/// The codes this gate has handed out, by code.
#[derive(Default)]
pub(crate) struct Pairings(Mutex<HashMap<String, Pairing>>);

impl Pairings {
    /// Hands out a code for `ask`: the code and the device code the agent polls with; none
    /// when too many wait.
    pub(crate) fn start(&self, ask: Ask, now: SystemTime) -> Option<(String, String)> {
        let device_code = CsrfToken::new_random_len(RANDOM_BYTES).into_secret();
        let mut all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        all.retain(|_, p| p.expires > now);
        if all.len() >= MOST_WAITING {
            return None;
        }
        let code = loop {
            let code = user_code();
            if !all.contains_key(&code) {
                break code;
            }
        };
        all.insert(
            code.clone(),
            Pairing {
                device_code: device_code.clone(),
                ask,
                expires: now + TTL,
                stage: Stage::Waiting,
            },
        );
        drop(all);
        Some((code, device_code))
    }

    /// What `code` asks for, while it waits for a confirm.
    pub(crate) fn waiting(&self, code: &str, now: SystemTime) -> Option<Ask> {
        let all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        all.get(code)
            .filter(|p| p.expires > now && matches!(p.stage, Stage::Waiting))
            .map(|p| p.ask.clone())
    }

    /// Confirms `code` with `enroll`, which runs once at most per code; `None` when the code
    /// is not waiting, else whether `enroll` worked. A failed enroll leaves the code waiting.
    pub(crate) fn confirm(
        &self,
        code: &str,
        now: SystemTime,
        enroll: impl FnOnce(&Ask) -> Result<Value, String>,
    ) -> Option<Result<Ask, String>> {
        let ask = self.begin(code, now)?;
        let enrolled = enroll(&ask);
        let stage = enrolled
            .as_ref()
            .map_or(Stage::Waiting, |value| Stage::Enrolled(value.clone()));
        if let Some(pairing) = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(code)
        {
            pairing.stage = stage;
        }
        Some(enrolled.map(|_| ask))
    }

    /// Moves `code` from waiting to enrolling: what it asks for, or `None` when it is not waiting.
    fn begin(&self, code: &str, now: SystemTime) -> Option<Ask> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(code)
            .filter(|p| p.expires > now && matches!(p.stage, Stage::Waiting))
            .map(|pairing| {
                pairing.stage = Stage::Enrolling;
                pairing.ask.clone()
            })
    }

    /// What the agent polling with `device_code` gets; a collected answer is gone after.
    pub(crate) fn poll(&self, device_code: &str, now: SystemTime) -> Polled {
        let mut all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(code) = all
            .iter()
            .find(|(_, p)| super::same(&p.device_code, device_code))
            .map(|(code, _)| code.clone())
        else {
            return Polled::Expired;
        };
        match all.remove(&code) {
            Some(p) if p.expires <= now => Polled::Expired,
            Some(Pairing {
                stage: Stage::Enrolled(value),
                ..
            }) => Polled::Enrolled(value),
            Some(p) => {
                all.insert(code, p);
                Polled::Pending
            }
            None => Polled::Expired,
        }
    }
}

/// A fresh `XXXX-XXXX` code of [`ALPHABET`] letters, each drawn without bias.
fn user_code() -> String {
    let mut letters = Vec::with_capacity(8);
    while letters.len() < 8 {
        let mut byte = [0u8; 1];
        OsRng.fill_bytes(&mut byte);
        let [b] = byte;
        if usize::from(b) < 240 {
            letters.push(ALPHABET.get(usize::from(b) % 20).copied().unwrap_or(b'X'));
        }
    }
    let (head, tail) = letters.split_at(4);
    format!(
        "{}-{}",
        String::from_utf8_lossy(head),
        String::from_utf8_lossy(tail)
    )
}

/// `typed` as a code is kept: upper case, with its dash.
fn normalize(typed: &str) -> String {
    let letters: String = typed
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    match letters.get(..4).zip(letters.get(4..)) {
        Some((head, tail)) => format!("{head}-{tail}"),
        None => letters,
    }
}

/// Serves `POST /machines/pair` and `POST /machines/pair/poll` for an agent, answering JSON.
pub(crate) fn agent(
    mut stream: TcpStream,
    leftover: Vec<u8>,
    length: Option<usize>,
    path: &str,
    login: Option<&Login>,
) {
    #[allow(
        clippy::disallowed_methods,
        reason = "code expiry; the store takes the time as an argument"
    )]
    let now = SystemTime::now();
    let asked = read_json(&mut stream, leftover, length).and_then(|body| {
        if path == "/machines/pair" {
            serde_json::from_value(body).map(Asked::Pair)
        } else {
            serde_json::from_value(body).map(Asked::Poll)
        }
        .map_err(|e| format!("the body is not a pairing request: {e}"))
    });
    let (status, body) = match (asked, login) {
        (Err(why), _) => ("400 Bad Request", json!({ "error": why })),
        (Ok(_), None) => (
            "501 Not Implemented",
            json!({"error": "this server pairs machines only with browser sign-in on; its operator must start the gate with --public-url"}),
        ),
        (Ok(Asked::Pair(ask)), Some(login)) => start(login, ask, now),
        (Ok(Asked::Poll(polled)), Some(login)) => poll(login, &polled.device_code, now),
    };
    respond(&mut stream, status, "application/json", &body.to_string());
}

/// What an agent posts.
enum Asked {
    /// A new code for this ask.
    Pair(Ask),
    /// Whether this code is confirmed.
    Poll(Poll),
}

/// A poll's body.
#[derive(Deserialize)]
struct Poll {
    /// The secret [`Pairings::start`] handed out.
    device_code: String,
}

/// Hands out a code for `ask`.
fn start(login: &Login, ask: Ask, now: SystemTime) -> (&'static str, Value) {
    let Some((code, device_code)) = login.pairings.start(ask, now) else {
        return (
            "429 Too Many Requests",
            json!({"error": "too many machines are pairing; try again in a few minutes"}),
        );
    };
    let uri = format!("{}{PATH}", login.public);
    (
        "200 OK",
        json!({"device_code": device_code, "user_code": code,
            "verification_uri_complete": format!("{uri}?code={code}"), "verification_uri": uri,
            "expires_in": TTL.as_secs(), "interval": INTERVAL_SECS}),
    )
}

/// Answers a poll the way RFC 8628's token endpoint does.
fn poll(login: &Login, device_code: &str, now: SystemTime) -> (&'static str, Value) {
    match login.pairings.poll(device_code, now) {
        Polled::Enrolled(value) => ("200 OK", value),
        Polled::Pending => ("400 Bad Request", json!({"error": "authorization_pending"})),
        Polled::Expired => ("400 Bad Request", json!({"error": "expired_token"})),
    }
}

/// The JSON body, when it fits and arrives whole.
fn read_json(
    stream: &mut TcpStream,
    mut body: Vec<u8>,
    length: Option<usize>,
) -> Result<Value, String> {
    let length = length.filter(|n| *n <= BODY_MAX).ok_or_else(|| {
        format!("send a JSON body with a Content-Length of at most {BODY_MAX} bytes")
    })?;
    body.truncate(length);
    let owed = u64::try_from(length.saturating_sub(body.len())).unwrap_or(0);
    stream
        .take(owed)
        .read_to_end(&mut body)
        .map_err(|e| format!("the body was cut off: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("the body is not JSON: {e}"))
}
