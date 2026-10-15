//! Client for the controller socket: runs one request through a configured daemon.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use moor_core::controller::{parse_server_frame, ClientFrame, ServerFrame, PROTOCOL};
use moor_core::{ErrorKind, Request, Response};

use crate::config_load::load_user;
use crate::env::Env;
use crate::error::CommandError;

pub mod remote;

/// Id carried by the single request of a session.
const REQUEST_ID: &str = "1";

/// Why a request could not be answered by the controller.
#[derive(Debug)]
pub enum ClientError {
    /// The socket could not be connected.
    Unreachable {
        /// Socket that was tried.
        socket: PathBuf,
        /// Connect failure.
        source: std::io::Error,
    },
    /// The controller broke the protocol or refused the session.
    Protocol(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable { socket, source } => {
                write!(
                    f,
                    "controller at {} is unreachable: {source}",
                    socket.display()
                )
            }
            Self::Protocol(msg) => write!(f, "controller protocol error: {msg}"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unreachable { source, .. } => Some(source),
            Self::Protocol(_) => None,
        }
    }
}

/// Controller socket to use: `MOOR_CONTROLLER`, else `controller.socket` of the user config.
///
/// A user config that fails to load yields `None`; the local path reports that error.
#[must_use]
pub fn configured_socket(env: &Env) -> Option<PathBuf> {
    let path = match env.get("MOOR_CONTROLLER").filter(|v| !v.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => load_user(env).ok()?.controller.socket?,
    };
    Some(env.expand_home(&path))
}

/// Run `request` on the controller at `socket`, passing progress messages to `on_progress`.
///
/// # Errors
/// [`ClientError::Unreachable`] when the connect fails, [`ClientError::Protocol`] for any
/// failure after it.
pub fn call(
    socket: &Path,
    request: &Request,
    mut on_progress: impl FnMut(&str),
) -> Result<Response, ClientError> {
    let stream = UnixStream::connect(socket).map_err(|source| ClientError::Unreachable {
        socket: socket.to_path_buf(),
        source,
    })?;
    let mut writer = stream
        .try_clone()
        .map_err(|e| ClientError::Protocol(format!("clone socket: {e}")))?;
    let mut reader = BufReader::new(stream);

    send(&mut writer, &ClientFrame::Hello { protocol: PROTOCOL })?;
    match next_frame(&mut reader)? {
        ServerFrame::Hello { protocol } if protocol == PROTOCOL => {}
        ServerFrame::Hello { protocol } => {
            return Err(ClientError::Protocol(format!(
                "controller speaks protocol {protocol}, this client speaks {PROTOCOL}"
            )));
        }
        other => return Err(unexpected(&other, "hello")),
    }

    send(
        &mut writer,
        &ClientFrame::Request {
            id: REQUEST_ID.to_owned(),
            request: request.clone(),
        },
    )?;
    loop {
        match next_frame(&mut reader)? {
            ServerFrame::Progress { id, message } if id == REQUEST_ID => on_progress(&message),
            ServerFrame::Response { id, envelope } if id == REQUEST_ID => return Ok(envelope),
            other => return Err(unexpected(&other, "progress or response")),
        }
    }
}

/// Failure envelope (exit class 1) for a request the controller could not answer.
#[must_use]
pub fn failure_response(cmd: &str, err: &ClientError) -> Response {
    let mut e = CommandError::new(ErrorKind::Failed, err.to_string());
    if let ClientError::Unreachable { .. } = err {
        e = e.with_hint(
            "start it with `moor controller serve --config <file>`, or unset MOOR_CONTROLLER \
             and controller.socket to run locally",
        );
    }
    e.into_response(cmd, Vec::new())
}

/// Write one frame and flush.
fn send(writer: &mut UnixStream, frame: &ClientFrame) -> Result<(), ClientError> {
    writer
        .write_all(frame.to_line().as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|e| ClientError::Protocol(format!("write to controller: {e}")))
}

/// Read and parse the next frame; EOF is a protocol error.
fn next_frame(reader: &mut impl BufRead) -> Result<ServerFrame, ClientError> {
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .map_err(|e| ClientError::Protocol(format!("read from controller: {e}")))?;
    if n == 0 {
        return Err(ClientError::Protocol(
            "controller closed the connection before answering".to_owned(),
        ));
    }
    parse_server_frame(line.trim_end())
        .map_err(|e| ClientError::Protocol(format!("malformed frame from controller: {e}")))
}

/// Error for a frame that does not fit the exchange; surfaces a controller error frame.
fn unexpected(frame: &ServerFrame, wanted: &str) -> ClientError {
    match frame {
        ServerFrame::Error { kind, message, .. } => ClientError::Protocol(format!(
            "controller rejected the session ({kind:?}): {message}"
        )),
        _ => ClientError::Protocol(format!("expected {wanted} frame, got another")),
    }
}

#[cfg(test)]
mod tests;
