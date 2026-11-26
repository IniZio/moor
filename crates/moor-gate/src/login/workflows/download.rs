//! One artifact's bytes as a download for the signed-in page.

use std::io::Write as _;
use std::net::TcpStream;

use moor_core::view::ArtifactView;
use serde_json::json;

use super::DOWNLOAD_MAX;
use crate::artifact::{self, Refusal, Wanted};
use crate::artifacts_http::Body;
use crate::login::account::json;
use crate::login::Session;
use crate::{log, Gate};

/// Answers `refusal` as a JSON error.
fn refuse(stream: &mut TcpStream, refusal: &Refusal) {
    json(
        stream,
        refusal.status(),
        &json!({"error": refusal.message()}),
    );
}

/// Sends the bytes of the artifact the query names, as a download, read as the session's person.
/// The bytes are streamed as the controller sends them, and a body that fails the controller's
/// integrity check is a `409`, or never arrives whole.
pub(crate) fn artifact(stream: &mut TcpStream, gate: &Gate, session: &Session, query: &str) {
    let pairs: std::collections::HashMap<String, String> =
        oauth2::url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
    let version = pairs.get("version").and_then(|v| v.parse().ok());
    let wanted = Wanted::parse(
        pairs.get("box").map_or("", String::as_str),
        pairs.get("name").map_or("", String::as_str),
        version,
    );
    let wanted = wanted.map(|w| Wanted {
        everyone: true,
        ..w
    });
    let found = wanted.and_then(|w| artifact::find(&gate.controller, &session.person, &w));
    let found = match found {
        Ok(found) => found,
        Err(refusal) => return refuse(stream, &refusal),
    };
    if found.record.size > DOWNLOAD_MAX {
        let why = Refusal::BadRequest("that artifact is too large to download here".to_owned());
        return refuse(stream, &why);
    }
    let mut body = Body {
        stream,
        record: &found.record,
        held: Vec::new(),
        started: false,
        head,
    };
    let read = artifact::read(&gate.controller, &session.person, &found, |data| {
        body.push(data)
    });
    match read {
        Ok(()) => body.finish(),
        Err(refusal) if !body.started => refuse(stream, &refusal),
        Err(refusal) => log(&format!(
            "a download of artifact {} ended short: {}",
            found.record.name,
            refusal.message()
        )),
    }
}

/// Writes the `200` head of a download of `record`. The bytes are the agent's: a download,
/// never a page, and sandboxed if a browser opens it.
fn head(stream: &mut TcpStream, record: &ArtifactView) -> std::io::Result<()> {
    let file: String = record
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"{file}\"\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: sandbox\r\nConnection: close\r\n\r\n",
        record.size
    )
}
