//! Client for the controller: runs one request through a configured daemon, reached over its
//! socket or through `ssh <host> moor controller attach`.
//!
//! A request that provably did not run (the transport failed before the hello reply, or the
//! controller answered `restarting`) is sent again within the redial budget. A session cut
//! after the request was written is an error, because the request may have run.

use std::cell::RefCell;
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use moor_client::Attempt;
use moor_core::controller::{
    parse_server_frame, ClientFrame, FrameErrorKind, ServerFrame, BYTES_MAX, PROTOCOL,
};
use moor_core::seam::proc::Tool;
use moor_core::{ErrorKind, Request, Response};

use crate::config_load::load_user;
use crate::env::Env;
use crate::error::CommandError;

mod interrupt;
mod redial;
pub mod remote;
pub mod watch;

use redial::redialing;

pub use watch::watch;

/// Id carried by the single request of a session.
const REQUEST_ID: &str = "1";

/// Polls of a failed ssh for its exit status, [`EXIT_POLL`] apart.
const EXIT_POLLS: u32 = 50;
/// Pause between exit status polls.
const EXIT_POLL: Duration = Duration::from_millis(10);

/// Where the controller is reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Unix socket of a local daemon.
    Socket(PathBuf),
    /// `ssh` destination that runs `moor controller attach`.
    Ssh(String),
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(path) => write!(f, "controller at {}", path.display()),
            Self::Ssh(host) => write!(f, "controller over ssh at {host}"),
        }
    }
}

/// Why a request could not be answered by the controller.
#[derive(Debug)]
pub enum ClientError {
    /// The controller could not be reached.
    Unreachable {
        /// Target that was tried.
        target: Target,
        /// Connect or spawn failure.
        detail: String,
    },
    /// The controller broke the protocol or refused the session.
    Protocol(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable { target, detail } => write!(f, "{target} is unreachable: {detail}"),
            Self::Protocol(msg) => write!(f, "controller protocol error: {msg}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// How a session ended without a response.
enum SessionError {
    /// The transport failed before the controller's hello reply.
    LostBeforeHello(String),
    /// The controller answered `restarting` before it ran the request.
    Restarting(String),
    /// Any other failure.
    Client(ClientError),
}

impl From<ClientError> for SessionError {
    fn from(e: ClientError) -> Self {
        Self::Client(e)
    }
}

/// Controller to use: `MOOR_CONTROLLER` (a socket), else `controller.ssh`, else
/// `controller.socket` of the user config.
///
/// A user config that fails to load yields `None`; the local path reports that error.
#[must_use]
pub fn configured_target(env: &Env) -> Option<Target> {
    let path = if let Some(p) = env.get("MOOR_CONTROLLER").filter(|v| !v.is_empty()) {
        PathBuf::from(p)
    } else {
        let controller = load_user(env).ok()?.controller;
        if let Some(host) = controller.ssh {
            return Some(Target::Ssh(host));
        }
        controller.socket?
    };
    Some(Target::Socket(env.expand_home(&path)))
}

/// Run `request` on the controller at `target`, passing progress messages to `on_progress` and
/// the raw bytes of each bytes frame, such as a `run` command's output, to `on_output`.
///
/// While a `run` waits on a controller socket, an interrupt cancels it and its reply still
/// arrives.
///
/// # Errors
/// [`ClientError::Unreachable`] when the transport cannot be opened (or, over ssh, dies before
/// the controller's hello reply), [`ClientError::Protocol`] for any other failure.
pub fn call(
    env: &Env,
    target: &Target,
    request: &Request,
    on_progress: impl FnMut(&str),
    mut on_output: impl FnMut(&[u8]),
) -> Result<Response, ClientError> {
    let forward = matches!(request, Request::Run(_));
    let on_progress = RefCell::new(on_progress);
    let progress = |msg: &str| (on_progress.borrow_mut())(msg);
    redialing(env, target, progress, || {
        with_transport(env, target, forward, |reader, mut writer| {
            session(reader, &mut writer, request, progress, &mut on_output)
        })
    })
}

/// Open the transport to `target` and run `exchange` over it; with `forward`, an interrupt
/// meanwhile is sent on to the controller.
fn with_transport<T>(
    env: &Env,
    target: &Target,
    forward: bool,
    exchange: impl FnOnce(&mut dyn BufRead, &mut dyn Write) -> Result<T, SessionError>,
) -> Attempt<T, ClientError> {
    match target {
        Target::Socket(socket) => {
            let stream = match UnixStream::connect(socket) {
                Ok(stream) => stream,
                Err(e) => {
                    return Attempt::Unrun(ClientError::Unreachable {
                        target: target.clone(),
                        detail: e.to_string(),
                    })
                }
            };
            let _forward = forward.then(|| interrupt::forward(&stream));
            let mut writer = match stream.try_clone() {
                Ok(writer) => writer,
                Err(e) => {
                    return Attempt::Failed(ClientError::Protocol(format!("clone socket: {e}")))
                }
            };
            match exchange(&mut BufReader::new(stream), &mut writer) {
                Ok(value) => Attempt::Done(value),
                Err(SessionError::LostBeforeHello(msg) | SessionError::Restarting(msg)) => {
                    Attempt::Unrun(ClientError::Protocol(msg))
                }
                Err(SessionError::Client(e)) => Attempt::Failed(e),
            }
        }
        Target::Ssh(host) => call_ssh(env, target, host, exchange),
    }
}

/// Transport over `ssh -T <host> moor controller attach`.
fn call_ssh<T>(
    env: &Env,
    target: &Target,
    host: &str,
    exchange: impl FnOnce(&mut dyn BufRead, &mut dyn Write) -> Result<T, SessionError>,
) -> Attempt<T, ClientError> {
    let unreachable = |detail: String| ClientError::Unreachable {
        target: target.clone(),
        detail,
    };
    let spawned = Command::new(ssh_program(env))
        .args(["-T", host, "moor", "controller", "attach"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return Attempt::Failed(unreachable(format!("cannot run ssh: {e}"))),
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Attempt::Failed(ClientError::Protocol("ssh pipes unavailable".to_owned()));
    };
    let result = exchange(&mut BufReader::new(stdout), &mut stdin);
    drop(stdin);
    let result = match result {
        Ok(value) => Attempt::Done(value),
        Err(SessionError::LostBeforeHello(msg)) => {
            Attempt::Unrun(unreachable(format!("{msg}{}", exit_note(&mut child))))
        }
        Err(SessionError::Restarting(msg)) => Attempt::Unrun(ClientError::Protocol(msg)),
        Err(SessionError::Client(e)) => Attempt::Failed(e),
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// ssh program: `MOOR_SSH_BIN`, then `tools.ssh` of the user config, then `ssh` on `PATH`.
fn ssh_program(env: &Env) -> PathBuf {
    if let Some(v) = env.get(Tool::Ssh.bin_var()).filter(|v| !v.is_empty()) {
        return v.into();
    }
    load_user(env)
        .ok()
        .and_then(|c| c.tools.ssh)
        .unwrap_or_else(|| Tool::Ssh.program().into())
}

/// `" (ssh <status>)"` once `child` has exited, else empty.
fn exit_note(child: &mut Child) -> String {
    for _ in 0..EXIT_POLLS {
        if let Ok(Some(status)) = child.try_wait() {
            return format!(" (ssh {status})");
        }
        std::thread::sleep(EXIT_POLL);
    }
    String::new()
}

/// Send the hello and check the controller's reply.
fn handshake(reader: &mut impl BufRead, writer: &mut impl Write) -> Result<(), SessionError> {
    send(writer, &ClientFrame::Hello { protocol: PROTOCOL })
        .map_err(SessionError::LostBeforeHello)?;
    let line = read_line(reader).map_err(SessionError::LostBeforeHello)?;
    match parse_frame(&line)? {
        ServerFrame::Hello { protocol } if protocol == PROTOCOL => Ok(()),
        ServerFrame::Hello { protocol } => Err(ClientError::Protocol(format!(
            "controller speaks protocol {protocol}, this client speaks {PROTOCOL}"
        ))
        .into()),
        other => Err(refused(&other, "hello")),
    }
}

/// The hello, request, progress and bytes exchange over an open transport.
///
/// Holds one bytes frame at a time. A `restarting` answer before any output leaves the request
/// unrun; after output it is an error, because the output may not come again.
fn session(
    mut reader: impl BufRead,
    writer: &mut impl Write,
    request: &Request,
    mut on_progress: impl FnMut(&str),
    mut on_output: impl FnMut(&[u8]),
) -> Result<Response, SessionError> {
    handshake(&mut reader, writer)?;

    send(
        writer,
        &ClientFrame::Request {
            id: REQUEST_ID.to_owned(),
            request: Box::new(request.clone()),
        },
    )
    .map_err(ClientError::Protocol)?;
    let mut buf = Vec::new();
    let mut output = false;
    loop {
        match next_frame(&mut reader)? {
            ServerFrame::Progress { id, message } if id == REQUEST_ID => on_progress(&message),
            ServerFrame::Bytes { id, len } if id == REQUEST_ID => {
                if len > BYTES_MAX {
                    return Err(ClientError::Protocol(format!(
                        "bytes frame of {len} exceeds {BYTES_MAX}"
                    ))
                    .into());
                }
                buf.resize(len, 0);
                reader
                    .read_exact(&mut buf)
                    .map_err(|e| ClientError::Protocol(format!("read from controller: {e}")))?;
                output = true;
                on_output(&buf);
            }
            ServerFrame::Response { id, envelope } if id == REQUEST_ID => return Ok(envelope),
            other if output => return Err(unexpected(&other, "progress, bytes or response").into()),
            other => return Err(refused(&other, "progress, bytes or response")),
        }
    }
}

/// Failure envelope (exit class 1) for a request the controller could not answer.
#[must_use]
pub fn failure_response(cmd: &str, err: &ClientError) -> Response {
    let mut e = CommandError::new(ErrorKind::Failed, err.to_string());
    if let ClientError::Unreachable { target, .. } = err {
        e = e.with_hint(match target {
            Target::Socket(_) => {
                "start it with `moor controller serve --config <file>`, or unset MOOR_CONTROLLER \
                 and controller.socket to run locally"
                    .to_owned()
            }
            Target::Ssh(host) => format!(
                "check `ssh {host} moor controller attach` works, or remove controller.ssh to \
                 run locally"
            ),
        });
    }
    e.into_response(cmd, Vec::new())
}

/// Write one frame and flush.
fn send(writer: &mut impl Write, frame: &ClientFrame) -> Result<(), String> {
    writer
        .write_all(frame.to_line().as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|e| format!("write to controller: {e}"))
}

/// Read one line; EOF is an error.
fn read_line(reader: &mut impl BufRead) -> Result<String, String> {
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .map_err(|e| format!("read from controller: {e}"))?;
    if n == 0 {
        return Err("controller closed the connection before answering".to_owned());
    }
    Ok(line)
}

/// Parse one received line.
fn parse_frame(line: &str) -> Result<ServerFrame, ClientError> {
    parse_server_frame(line.trim_end())
        .map_err(|e| ClientError::Protocol(format!("malformed frame from controller: {e}")))
}

/// Read and parse the next frame; EOF is a protocol error.
fn next_frame(reader: &mut impl BufRead) -> Result<ServerFrame, ClientError> {
    parse_frame(&read_line(reader).map_err(ClientError::Protocol)?)
}

/// [`unexpected`], except that a `restarting` frame means the request did not run.
fn refused(frame: &ServerFrame, wanted: &str) -> SessionError {
    match frame {
        ServerFrame::Error {
            kind: FrameErrorKind::Restarting,
            message,
            ..
        } => SessionError::Restarting(message.clone()),
        other => unexpected(other, wanted).into(),
    }
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
