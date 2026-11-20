//! `POST /hooks/github`: the App's webhook. It sits outside sign-in, since GitHub signs each
//! delivery instead. The gate passes the headers it needs and the body byte for byte to the
//! controller, which holds the secret, checks the signature and answers what to tell GitHub.

use std::io::Read as _;
use std::net::TcpStream;

use moor_core::request::{GitHubHookReq, GitHubHookValue, Request as ControllerRequest};
use moor_core::ExitClass;

use crate::http::{header, respond, unavailable};
use crate::tools::client_down;
use crate::{log, Gate};

/// The route.
pub(crate) const PATH: &str = "/hooks/github";

/// Largest body taken; GitHub's `pull_request` payloads are far smaller.
const BODY_MAX: usize = 1 << 20;

/// Serves one delivery whose body starts with `leftover`.
pub(crate) fn serve(
    mut stream: TcpStream,
    leftover: Vec<u8>,
    req: &httparse::Request<'_, '_>,
    gate: &Gate,
) {
    let Some(length) = header(req, "content-length").and_then(|v| v.trim().parse::<usize>().ok())
    else {
        return respond(
            &mut stream,
            "411 Length Required",
            "text/plain",
            "send Content-Length\n",
        );
    };
    if length > BODY_MAX {
        return respond(
            &mut stream,
            "413 Payload Too Large",
            "text/plain",
            "body over 1 MiB\n",
        );
    }
    let Some(body) = read_body(&mut stream, leftover, length) else {
        return respond(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            "body cut short\n",
        );
    };
    let Ok(body) = String::from_utf8(body) else {
        return respond(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            "body is not UTF-8\n",
        );
    };
    let hook = GitHubHookReq {
        event: header(req, "x-github-event")
            .unwrap_or_default()
            .trim()
            .to_owned(),
        delivery: header(req, "x-github-delivery").map(|v| v.trim().to_owned()),
        signature: header(req, "x-hub-signature-256").map(str::to_owned),
        body,
    };
    answer(&mut stream, gate, hook);
}

/// Hands `hook` to the controller and answers GitHub as it says.
fn answer(stream: &mut TcpStream, gate: &Gate, hook: GitHubHookReq) {
    match gate
        .controller
        .request(ControllerRequest::GitHubHook(hook), |_| {})
    {
        Ok(reply) if reply.class == ExitClass::Ok => {
            match reply.value.map(serde_json::from_value::<GitHubHookValue>) {
                Some(Ok(GitHubHookValue::Accepted)) => {
                    respond(stream, "202 Accepted", "text/plain", "accepted\n");
                }
                Some(Ok(GitHubHookValue::BadSignature)) => respond(
                    stream,
                    "401 Unauthorized",
                    "text/plain",
                    "bad or missing X-Hub-Signature-256\n",
                ),
                Some(Ok(GitHubHookValue::Off)) => {
                    respond(stream, "404 Not Found", "text/plain", "not found\n");
                }
                _ => bad_gateway(
                    stream,
                    "the controller answered in a shape the gate does not know",
                ),
            }
        }
        Ok(reply) if reply.class == ExitClass::Usage => {
            let why = reply.error.map_or_else(String::new, |e| e.message);
            respond(stream, "400 Bad Request", "text/plain", &format!("{why}\n"));
        }
        Ok(reply) => {
            let why = reply.error.map_or_else(String::new, |e| e.message);
            bad_gateway(stream, &why);
        }
        Err(e) if client_down(&e) => unavailable(stream),
        Err(e) => bad_gateway(stream, &e.to_string()),
    }
}

/// Answers `502` and logs `why`.
fn bad_gateway(stream: &mut TcpStream, why: &str) {
    log(&format!("github hook: {why}"));
    respond(
        stream,
        "502 Bad Gateway",
        "text/plain",
        "the controller could not take it\n",
    );
}

/// The `length` bytes of body that start with `body`, when they all arrive.
fn read_body(stream: &mut TcpStream, mut body: Vec<u8>, length: usize) -> Option<Vec<u8>> {
    body.truncate(length);
    let owed = u64::try_from(length.saturating_sub(body.len())).ok()?;
    stream.take(owed).read_to_end(&mut body).ok()?;
    (body.len() == length).then_some(body)
}
