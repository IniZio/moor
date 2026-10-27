//! The `attach` request: hands the connection to the terminal relay.

use moor_core::request::Grant;
use moor_core::{AttachReq, ErrorKind, Response};

use super::{Duplex, Peer};
use crate::controller::relay;
use crate::env::Env;
use crate::error::CommandError;
use crate::Moor;

/// Relay the agent's terminal on the Box `req` names over this connection, then answer.
pub(super) fn attach(
    env: &Env,
    peer: &Peer,
    grant: Grant,
    id: &str,
    req: &AttachReq,
    (input, output): Duplex<'_>,
) -> Response {
    let refused = |e: CommandError| e.into_response("attach", vec![]);
    let Some(hangup) = peer.hangup.as_ref() else {
        return refused(CommandError::new(
            ErrorKind::Failed,
            "this connection cannot be relayed",
        ));
    };
    let argv = match Moor::from_env(env).and_then(|m| m.granted(grant).attach_argv(req)) {
        Ok(argv) => argv,
        Err(e) => return refused(e),
    };
    let drain = peer.conn.drain();
    let link = relay::Link {
        id,
        input,
        output,
        unblock: &|| hangup.stop_reading(),
        stopping: &|| drain.begun(),
    };
    match relay::run(&argv, req.cols, req.rows, link) {
        Ok(view) => Response::ok("attach", serde_json::to_value(view).ok()),
        Err(e) => refused(CommandError::new(
            ErrorKind::Failed,
            format!("cannot relay the agent's terminal: {e}"),
        )),
    }
}
