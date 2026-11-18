//! One artifact's bytes as a download for the signed-in page.

use std::net::TcpStream;

use serde_json::json;

use super::DOWNLOAD_MAX;
use crate::login::account::json;
use crate::login::Session;
use crate::Gate;

/// Sends the bytes of the artifact the query names, as a download, read as the session's person.
pub(crate) fn artifact(stream: &mut TcpStream, gate: &Gate, session: &Session, query: &str) {
    use std::io::Write as _;

    use crate::artifact::{self, Wanted};

    let pairs: std::collections::HashMap<String, String> =
        oauth2::url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
    let version = pairs.get("version").and_then(|v| v.parse().ok());
    let refuse = |stream: &mut TcpStream, refusal: &artifact::Refusal| {
        json(
            stream,
            refusal.status(),
            &json!({"error": refusal.message()}),
        );
    };
    let wanted = Wanted::parse(
        pairs.get("box").map_or("", String::as_str),
        pairs.get("name").map_or("", String::as_str),
        version,
    );
    let found = wanted.and_then(|w| artifact::find(&gate.controller, &session.person, &w));
    let found = match found {
        Ok(found) => found,
        Err(refusal) => return refuse(stream, &refusal),
    };
    if found.record.size > DOWNLOAD_MAX {
        let why =
            artifact::Refusal::BadRequest("that artifact is too large to download here".to_owned());
        return refuse(stream, &why);
    }
    let mut bytes = Vec::new();
    let read = artifact::read(&gate.controller, &session.person, &found, |data| {
        bytes.extend_from_slice(data);
        Ok(())
    });
    if let Err(refusal) = read {
        return refuse(stream, &refusal);
    }
    // The bytes are the agent's: a download, never a page, and sandboxed if a browser opens it.
    let file: String = found
        .record
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
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"{file}\"\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: sandbox\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    let _ = stream.write_all(&bytes);
}
