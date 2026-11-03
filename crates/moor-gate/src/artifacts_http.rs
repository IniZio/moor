//! `GET /boxes/{box}/artifacts/{name}` and `.../{name}/{version}`: the bytes of one Artifact a
//! Box published, over plain HTTP for a token with `moor:read`, streamed as the controller sends
//! them. The Box name keeps its slash, literal or as `%2F`.

use std::io::{self, Write};
use std::net::TcpStream;

use moor_core::view::ArtifactView;

use crate::artifact::{self, Refusal, Wanted, ARTIFACT_INLINE_MAX};
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
    let mut body = Body {
        stream: &mut stream,
        record: &found.record,
        held: Vec::new(),
        started: false,
    };
    let read = artifact::read(socket, &person, &found, |data| body.push(data));
    match read {
        Ok(()) => body.finish(),
        Err(refusal) if !body.started => refuse(&mut stream, &refusal),
        Err(refusal) => log(&format!(
            "a download of artifact {} ended short: {}",
            found.record.name,
            refusal.message()
        )),
    }
}

/// The body on its way out. Up to [`ARTIFACT_INLINE_MAX`] bytes wait before the head is
/// written, so a mismatch in a small Artifact is still a 409; past that the last byte waits,
/// so a body that fails the check never arrives whole.
struct Body<'a> {
    /// The connection.
    stream: &'a mut TcpStream,
    /// The Artifact being sent.
    record: &'a ArtifactView,
    /// Bytes received and not yet written.
    held: Vec<u8>,
    /// Whether the head went out.
    started: bool,
}

impl Body<'_> {
    /// Takes one piece from the controller.
    fn push(&mut self, data: &[u8]) -> io::Result<()> {
        self.held.extend_from_slice(data);
        if !self.started && self.held.len() as u64 <= ARTIFACT_INLINE_MAX {
            return Ok(());
        }
        if !self.started {
            self.started = true;
            head(self.stream, self.record)?;
        }
        let keep = self.held.split_off(self.held.len().saturating_sub(1));
        self.stream.write_all(&self.held)?;
        self.held = keep;
        Ok(())
    }

    /// Writes what is held once the controller vouched for the whole body.
    fn finish(self) {
        if !self.started && head(self.stream, self.record).is_err() {
            return;
        }
        let _ = self.stream.write_all(&self.held);
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
