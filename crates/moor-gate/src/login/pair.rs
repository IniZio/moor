//! Pairing a machine by code, in the shape of OAuth's device authorization grant (RFC 8628):
//! the agent posts its key to `/machines/pair` and gets a short code, its owner confirms the code
//! at `/pair` behind browser sign-in, and the agent's poll at `/machines/pair/poll` gets the
//! enrollment the confirm made. The controller keeps every code, so the agent's polls and the
//! confirm may each land on a different gate replica.

use std::io::Read;
use std::net::TcpStream;
use std::time::Duration;

use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::OsRng;
use moor_client::Client;
use moor_core::request::{MachineEnrollReq, MachinePairReq, PairOp, Request as ControllerRequest};
use moor_core::response::ExitClass;
use moor_core::view::user::MachinePairValue;
use oauth2::CsrfToken;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Login, RANDOM_BYTES};
use crate::http::respond;
use crate::token::Person;
use crate::{tools, Gate};

pub(crate) mod page;

/// The confirm page's path.
pub(crate) const PATH: &str = "/pair";

/// How long a code can be confirmed and its answer collected.
const TTL: Duration = Duration::from_mins(10);

/// Seconds the agent waits between polls.
pub(super) const INTERVAL_SECS: u64 = 5;

/// Codes drawn before giving up when each is already taken.
const TRIES: usize = 4;

/// Letters a code is made of: no vowels, so no word, and none that reads as a digit.
const ALPHABET: &[u8; 20] = b"BCDFGHJKLMNPQRSTVWXZ";

/// Largest JSON body the agent routes read.
const BODY_MAX: usize = 16 * 1024;

/// What the agent asks to be enrolled as.
#[derive(Clone, Serialize, Deserialize)]
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
    /// The provider the machine offers Boxes on, when it hosts a Box pool.
    #[serde(default)]
    pool: Option<String>,
}

/// Sends `op` to the controller, which keeps every code so any gate replica can answer for
/// it: where the code stands, or why the controller could not say.
pub(crate) fn step(controller: &Client, op: PairOp) -> Result<MachinePairValue, String> {
    let reply = controller
        .request(
            ControllerRequest::MachinePair(MachinePairReq { op }),
            |_| {},
        )
        .map_err(|e| format!("moor could not be reached: {e}"))?;
    if reply.class != ExitClass::Ok {
        return Err(reply
            .error
            .map_or_else(|| "moor refused the pairing".to_owned(), |e| e.message));
    }
    serde_json::from_value(reply.value.unwrap_or_default())
        .map_err(|e| format!("moor answered in a shape the gate does not know: {e}"))
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
    gate: &Gate,
    login: Option<&Login>,
) {
    let asked = read_json(&mut stream, leftover, length).and_then(|mut body| {
        let token = if path == "/machines/pair" {
            take_token(&mut body)?
        } else {
            None
        };
        if path == "/machines/pair" {
            serde_json::from_value(body).map(|ask| Asked::Pair(ask, token))
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
        (Ok(Asked::Pair(ask, None)), Some(login)) => start(&gate.controller, login, &ask),
        (Ok(Asked::Pair(ask, Some(token))), Some(_)) => redeem(gate, &ask, token),
        (Ok(Asked::Poll(polled)), Some(_)) => poll(&gate.controller, polled.device_code),
    };
    respond(&mut stream, status, "application/json", &body.to_string());
}

/// What an agent posts.
enum Asked {
    /// A new code for this ask, or an enrollment at once for the portal token it carries.
    Pair(Ask, Option<String>),
    /// Whether this code is confirmed.
    Poll(Poll),
}

/// A poll's body.
#[derive(Deserialize)]
struct Poll {
    /// The secret [`start`] handed out.
    device_code: String,
}

/// Takes the portal token off a pairing body, so it never reaches the stored [`Ask`].
fn take_token(body: &mut Value) -> Result<Option<String>, String> {
    match body.as_object_mut().and_then(|o| o.remove("token")) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(token)) => Ok(Some(token)),
        Some(_) => Err("the token must be a string".to_owned()),
    }
}

/// The `machine-enroll` that adds the machine `ask` describes.
pub(crate) fn enroll_request(ask: &Ask) -> ControllerRequest {
    ControllerRequest::MachineEnroll(MachineEnrollReq {
        identity: None,
        email: None,
        name: ask.name.clone(),
        pubkey: ask.pubkey.clone(),
        labels: ask.labels.clone(),
        capacity: ask.slots,
        pool: ask.pool.clone(),
    })
}

/// Enrolls `ask`'s machine as the person whose grant `token` redeems, with no confirm page. The
/// controller lets one redeem take a grant, so a token enrolls once.
fn redeem(gate: &Gate, ask: &Ask, token: String) -> (&'static str, Value) {
    match step(&gate.controller, PairOp::Redeem { token }) {
        Ok(MachinePairValue::Granted { owner }) => {
            let person = Person {
                identity: owner.identity,
                email: owner.email,
            };
            match tools::act_as(&gate.controller, &person, enroll_request(ask)) {
                Ok(enrolled) => ("200 OK", json!({ "enrolled": enrolled })),
                Err(why) => unreachable_controller(&why),
            }
        }
        Ok(MachinePairValue::Gone) => (
            "400 Bad Request",
            json!({"error": "this token is unknown, used or expired; make a new one on the Machines page"}),
        ),
        Ok(other) => unreachable_controller(&format!("moor answered {other:?}")),
        Err(why) => unreachable_controller(&why),
    }
}

/// Hands out a code for `ask`, kept by the controller.
fn start(controller: &Client, login: &Login, ask: &Ask) -> (&'static str, Value) {
    let Ok(ask) = serde_json::to_value(ask) else {
        return unreachable_controller("the pairing could not be encoded");
    };
    for _ in 0..TRIES {
        let code = user_code();
        let device_code = CsrfToken::new_random_len(RANDOM_BYTES).into_secret();
        let op = PairOp::Start {
            code: code.clone(),
            device_code: device_code.clone(),
            ask: ask.clone(),
            ttl_secs: TTL.as_secs(),
        };
        match step(controller, op) {
            Ok(MachinePairValue::Started) => {
                let uri = format!("{}{PATH}", login.public);
                return (
                    "200 OK",
                    json!({"device_code": device_code, "user_code": code,
                        "verification_uri_complete": format!("{uri}?code={code}"),
                        "verification_uri": uri, "expires_in": TTL.as_secs(),
                        "interval": login.pair_interval}),
                );
            }
            Ok(MachinePairValue::Full) => {
                return (
                    "429 Too Many Requests",
                    json!({"error": "too many machines are pairing; try again in a few minutes"}),
                )
            }
            Ok(MachinePairValue::Taken) => {}
            Ok(other) => return unreachable_controller(&format!("moor answered {other:?}")),
            Err(why) => return unreachable_controller(&why),
        }
    }
    unreachable_controller("every code drawn was taken")
}

/// Answers a poll the way RFC 8628's token endpoint does. A controller that cannot answer is a
/// `502`, never `expired_token`, so the agent does not take it for an expired code.
fn poll(controller: &Client, device_code: String) -> (&'static str, Value) {
    match step(controller, PairOp::Poll { device_code }) {
        Ok(MachinePairValue::Enrolled { enrolled }) => ("200 OK", enrolled),
        Ok(MachinePairValue::Pending) => {
            ("400 Bad Request", json!({"error": "authorization_pending"}))
        }
        Ok(MachinePairValue::Gone) => ("400 Bad Request", json!({"error": "expired_token"})),
        Ok(other) => unreachable_controller(&format!("moor answered {other:?}")),
        Err(why) => unreachable_controller(&why),
    }
}

/// A `502` for a pairing the controller could not serve.
fn unreachable_controller(why: &str) -> (&'static str, Value) {
    crate::log(&format!("pairing a machine failed: {why}"));
    ("502 Bad Gateway", json!({ "error": why }))
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
