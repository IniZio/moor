//! `POST /machines/enroll`: a machine agent trades its owner's id token for a machine id.

use std::io::Read;
use std::net::TcpStream;

use moor_client::Client;
use moor_core::request::{MachineEnrollReq, Request};
use moor_core::response::ExitClass;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::http::respond;
use crate::{rewrite, Gate};

/// Largest request body the route reads.
const BODY_MAX: usize = 16 * 1024;

/// What the agent posts.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Enrollment {
    /// The owner's OIDC id token for the gate's client.
    id_token: String,
    /// The agent's ed25519 public key as an OpenSSH line.
    pubkey: String,
    /// The machine's name.
    name: String,
    /// The machine's labels.
    #[serde(default)]
    labels: Vec<String>,
    /// How many Boxes may hold the machine at once.
    #[serde(default = "one")]
    slots: u16,
}

/// The slot count of an agent that names none.
const fn one() -> u16 {
    1
}

/// Answers one enrollment with the controller's value, or `{"error": ...}` and a status.
pub(crate) fn handle(mut stream: TcpStream, leftover: Vec<u8>, length: Option<usize>, gate: &Gate) {
    let (status, body) = match enroll(&mut stream, leftover, length, gate) {
        Ok(value) => ("200 OK", value),
        Err((status, why)) => (status, json!({ "error": why })),
    };
    respond(&mut stream, status, "application/json", &body.to_string());
}

/// Reads the body, verifies its id token, and enrolls the machine as that token's person.
fn enroll(
    stream: &mut TcpStream,
    mut body: Vec<u8>,
    length: Option<usize>,
    gate: &Gate,
) -> Result<Value, (&'static str, String)> {
    let length = length.filter(|n| *n <= BODY_MAX).ok_or_else(|| {
        (
            "413 Content Too Large",
            format!("send a JSON body with a Content-Length of at most {BODY_MAX} bytes"),
        )
    })?;
    body.truncate(length);
    let owed = u64::try_from(length - body.len()).unwrap_or(0);
    stream
        .take(owed)
        .read_to_end(&mut body)
        .map_err(|e| ("400 Bad Request", format!("the body was cut off: {e}")))?;
    let got: Enrollment = serde_json::from_slice(&body).map_err(|e| {
        (
            "400 Bad Request",
            format!("the body is not an enrollment: {e}"),
        )
    })?;
    let person = gate
        .verifier
        .verify_id_token(&got.id_token)
        .map_err(|why| ("401 Unauthorized", why))?;
    let request = Request::MachineEnroll(MachineEnrollReq {
        identity: None,
        email: None,
        name: got.name,
        pubkey: got.pubkey,
        labels: got.labels,
        capacity: got.slots,
    });
    let acting = rewrite::act(request, &person).map_err(|why| ("403 Forbidden", why))?;
    let reply = Client::new(&gate.front_socket)
        .request(acting, |_| {})
        .map_err(|e| {
            crate::log(&format!("enrolling a machine failed: {e}"));
            (
                "502 Bad Gateway",
                "the controller is unreachable".to_owned(),
            )
        })?;
    if reply.class != ExitClass::Ok {
        let why = reply
            .error
            .map_or_else(|| "the controller refused".to_owned(), |e| e.message);
        return Err(("422 Unprocessable Entity", why));
    }
    Ok(reply.value.unwrap_or_default())
}
