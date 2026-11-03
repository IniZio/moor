//! `GET /boxes/{box}/artifacts/{name}` and `.../{name}/{version}`: the bytes of one Artifact a
//! Box published, over plain HTTP for a token with `moor:read`, streamed as the controller sends
//! them. The Box name keeps its slash, literal or as `%2F`.

use std::io::{self, Write};
use std::net::TcpStream;

use moor_core::view::ArtifactView;

use crate::artifact::{self, Refusal, Wanted};
use crate::events_http::{error, reader, reply};
use crate::{log, Gate};

/// Every Artifact path starts with this.
pub(crate) const PREFIX: &str = "/boxes/";

/// Content type sent when the recorded one is not a safe header value.
const FALLBACK_TYPE: &str = "application/octet-stream";

/// Answers one `GET` of `path`, which starts with [`PREFIX`].
pub(crate) fn serve(
    mut stream: TcpStream,
    path: &str,
    req: &httparse::Request<'_, '_>,
    gate: &Gate,
) {
    let Some((mcp, person)) = reader(&mut stream, req, gate, "/boxes") else {
        return;
    };
    let socket = mcp.front_socket();
    let found = parse(path).and_then(|wanted| artifact::find(socket, &person, &wanted));
    let found = match found {
        Ok(found) => found,
        Err(refusal) => return refuse(&mut stream, &refusal),
    };
    let mut started = false;
    let read = artifact::read(socket, &person, &found, |data| {
        if !started {
            started = true;
            head(&mut stream, &found.record)?;
        }
        stream.write_all(data)
    });
    match read {
        Ok(()) if !started => {
            let _ = head(&mut stream, &found.record);
        }
        Ok(()) => {}
        Err(refusal) if !started => refuse(&mut stream, &refusal),
        Err(refusal) => log(&format!(
            "a download of artifact {} stopped part way: {}",
            found.record.name,
            refusal.message()
        )),
    }
}

/// The Artifact `path` names, or why it names none.
fn parse(path: &str) -> Result<Wanted, Refusal> {
    let rest = path
        .strip_prefix(PREFIX)
        .unwrap_or_default()
        .replace("%2F", "/")
        .replace("%2f", "/");
    let segments: Vec<&str> = rest.split('/').collect();
    match segments.as_slice() {
        [r#box @ .., "artifacts", name, version] if is_number(version) => {
            let version = version
                .parse()
                .map_err(|_| Refusal::BadRequest(format!("bad version {version}")))?;
            Wanted::parse(&r#box.join("/"), name, Some(version))
        }
        [r#box @ .., "artifacts", name] => Wanted::parse(&r#box.join("/"), name, None),
        _ => Err(Refusal::NotFound("not found".to_owned())),
    }
}

/// Whether `s` is all ASCII digits, and not empty.
fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Writes the `200` head for `record`: its type, length and sha256 as the `ETag`.
fn head(stream: &mut TcpStream, record: &ArtifactView) -> io::Result<()> {
    let content_type =
        if record.content_type.is_empty() || record.content_type.chars().any(char::is_control) {
            FALLBACK_TYPE
        } else {
            &record.content_type
        };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nETag: \"{}\"\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: sandbox\r\nConnection: close\r\n\r\n",
        record.size, record.sha256
    )
}

/// Answers `refusal` as a JSON error.
fn refuse(stream: &mut TcpStream, refusal: &Refusal) {
    reply(stream, refusal.status(), None, &error(refusal.message()));
}
