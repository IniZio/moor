//! Splits the client's byte stream into frames and makes each act as the connection's person.

use moor_core::controller::{parse_frame, ClientFrame, ServerFrame};
use moor_core::request::{ActAsReq, EnrollReq, Request};
use moor_core::response::{ErrorKind, ErrorView, Response};

use crate::Person;

/// Longest frame line the gate buffers; a longer one ends the connection.
const LINE_MAX: usize = 1024 * 1024;

/// The client's stream so far: a partial line, or raw bytes still owed to a bytes frame.
#[derive(Default)]
pub(crate) struct Splitter {
    /// The current line without its newline.
    line: Vec<u8>,
    /// Raw bytes of a bytes frame still to pass through.
    raw: usize,
}

impl Splitter {
    /// Takes `chunk` of the client's stream, appending what goes to the controller to `up` and
    /// the frames the gate answers itself to `back`.
    pub(crate) fn feed(
        &mut self,
        mut chunk: &[u8],
        person: &Person,
        up: &mut Vec<u8>,
        back: &mut Vec<String>,
    ) -> Result<(), String> {
        while !chunk.is_empty() {
            if self.raw > 0 {
                let (raw, rest) = chunk.split_at(self.raw.min(chunk.len()));
                up.extend_from_slice(raw);
                self.raw -= raw.len();
                chunk = rest;
                continue;
            }
            let Some(end) = chunk.iter().position(|&b| b == b'\n') else {
                self.line.extend_from_slice(chunk);
                if self.line.len() > LINE_MAX {
                    return Err(format!("a frame line is longer than {LINE_MAX} bytes"));
                }
                return Ok(());
            };
            let (head, rest) = chunk.split_at(end);
            self.line.extend_from_slice(head);
            chunk = rest.get(1..).unwrap_or_default();
            let line = std::mem::take(&mut self.line);
            let line = String::from_utf8_lossy(&line);
            match frame(&line, person) {
                Ok((forward, raw)) => {
                    up.extend_from_slice(forward.to_line().as_bytes());
                    self.raw = raw;
                }
                Err(reply) => back.push(reply.to_line()),
            }
        }
        Ok(())
    }
}

/// The frame to send on for `line`, and how many raw bytes follow it; or the gate's answer.
fn frame(line: &str, person: &Person) -> Result<(ClientFrame, usize), Box<ServerFrame>> {
    match parse_frame(line).map_err(Box::new)? {
        ClientFrame::Request { id, request } => {
            let cmd = request.cmd();
            match act(*request, person) {
                Ok(request) => Ok((
                    ClientFrame::Request {
                        id,
                        request: Box::new(request),
                    },
                    0,
                )),
                Err(message) => Err(Box::new(refused(id, cmd, message))),
            }
        }
        ClientFrame::Watch {
            identity: Some(other),
            ..
        } if other != person.identity => Err(Box::new(ServerFrame::Error {
            id: None,
            kind: moor_core::controller::FrameErrorKind::Malformed,
            message: not_you(person, &other),
        })),
        ClientFrame::Watch {
            since,
            boxes,
            kinds,
            parent,
            follow,
            ..
        } => Ok((
            ClientFrame::Watch {
                since,
                boxes,
                kinds,
                parent,
                follow,
                identity: Some(person.identity.clone()),
            },
            0,
        )),
        frame @ ClientFrame::Bytes { len, .. } => Ok((frame, len)),
        frame @ (ClientFrame::Hello { .. } | ClientFrame::Resize { .. }) => Ok((frame, 0)),
    }
}

/// `request` acting as `person`, with their verified email on an `enroll`.
pub(crate) fn act(request: Request, person: &Person) -> Result<Request, String> {
    let (asserted, inner) = request.split_identity()?;
    if let Some(other) = asserted.filter(|id| *id != person.identity) {
        return Err(not_you(person, &other));
    }
    let inner = match inner {
        Request::Enroll(EnrollReq {
            email: Some(email), ..
        }) if email != person.email => {
            return Err(format!(
                "this connection's verified email is {}, not {email}",
                person.email
            ));
        }
        Request::Enroll(EnrollReq { identity, .. }) => Request::Enroll(EnrollReq {
            identity,
            email: Some(person.email.clone()),
        }),
        other => other,
    };
    Ok(Request::As(ActAsReq {
        identity: person.identity.clone(),
        request: Box::new(inner),
    }))
}

/// Why a frame asserting `other` is refused.
fn not_you(person: &Person, other: &moor_core::user::Identity) -> String {
    format!(
        "this connection acts only as {}, not {other}",
        person.identity
    )
}

/// The answer to request `id` the gate refused.
fn refused(id: String, cmd: &str, message: String) -> ServerFrame {
    ServerFrame::Response {
        id,
        envelope: Response::err(
            cmd,
            ErrorView {
                kind: ErrorKind::Refused,
                message,
                hints: Vec::new(),
                context: Vec::new(),
                detail: None,
            },
        ),
    }
}

#[cfg(test)]
mod tests;
