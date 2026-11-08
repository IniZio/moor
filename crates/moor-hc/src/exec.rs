//! `moor-hc exec`: a byte pipe to a command in an up Box, the transport hc's `--via` needs.
//!
//! It refuses a Box that is not up, because the controller's exec wakes an asleep Box and hc
//! keeps redialing a machine it lost: a Box put to sleep would wake again at once.

use std::io::{Read, Write};

use moor_client::Client;
use moor_core::request::{BoxTarget, ExecReq, Request};
use moor_core::view::ExecView;
use moor_core::BoxName;

use crate::boxes::is_up;
use crate::error::Error;

/// Runs `argv` in Box `name` and returns its exit code.
///
/// `stdin` goes in, and the command's stdout and stderr come back apart, untouched. The code is
/// `None` when the command was killed or the link dropped before it exited.
pub fn run(
    client: &Client,
    name: &str,
    argv: Vec<String>,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<Option<i32>, Error> {
    let target = BoxName::parse_allow_bare(name).map_err(|e| Error::Usage(e.to_string()))?;
    if !is_up(client, name)? {
        return Err(Error::Refused(format!(
            "Box {name} is not up; moor-hc exec does not wake it"
        )));
    }
    let req = Request::Exec(ExecReq {
        target: BoxTarget::Name(target),
        argv,
        timeout_secs: None,
    });
    let reply = client.pipe(req, stdin, stdout, stderr)?;
    if let Some(e) = reply.error {
        return Err(Error::Refused(e.message));
    }
    let view: ExecView = serde_json::from_value(reply.value.unwrap_or_default())?;
    Ok(view.exit)
}
