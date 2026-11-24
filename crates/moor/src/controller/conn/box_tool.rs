//! The `box-tool` request: a claimed agent place's `exec_in_box`, `exec_wait`, `start_box` or
//! `close_box`, run as the place's user once [`Moor::admit_box_tool`] allows it.
//!
//! A command runs on past its call: the call answers within [`Moor::BOX_RUN_WINDOW`] with the
//! output so far, and `exec_wait` reads on.

use std::sync::Arc;

use moor_core::request::{BoxCall, BoxRunId, BoxToolReq};
use moor_core::user::{Identity, Viewer};
use moor_core::{ErrorKind, Request, Response};

use super::{respond_unless_draining, Acting, Duplex, Peer};
use crate::controller::progress::{wait_unless_draining, WAIT};
use crate::env::Env;
use crate::error::CommandError;
use crate::Moor;

/// Answers `req`: refused, the command's output so far, or the start or close it makes. None
/// when the drain began before the answer.
pub(super) fn box_tool(
    env: &Env,
    peer: &Peer,
    id: &str,
    asserted: Option<&Identity>,
    req: &BoxToolReq,
    (_input, out): Duplex<'_>,
) -> Option<Response> {
    let admitted = Moor::from_env(env).and_then(|m| m.admit_box_tool(&peer.caller, asserted, req));
    let (grant, who, request) = match admitted {
        Err(e) => return Some(e.into_response("box-tool", vec![])),
        Ok(admitted) => admitted,
    };
    let Viewer::User(user) = grant.viewer else {
        let e = CommandError::new(ErrorKind::Internal, "a place's Box tool acts for no user");
        return Some(e.into_response("box-tool", vec![]));
    };
    let drain = peer.conn.drain();
    let (answering, draining, stopping) = (env.clone(), Arc::clone(drain), Arc::clone(drain));
    let answer = move |run: Result<BoxRunId, CommandError>| {
        let reply = run.and_then(|run| {
            Moor::from_env(&answering)?
                .answer_box_run(user, run, Moor::BOX_RUN_WINDOW, &|| draining.begun())
        });
        match reply {
            Ok(view) => Response::ok("box-tool", serde_json::to_value(view).ok()),
            Err(e) => e.into_response("box-tool", vec![]),
        }
    };
    let acting = Acting {
        who: Some(who),
        grant: grant.clone(),
    };
    match request {
        Request::Exec(exec) => {
            let start = env.clone();
            wait_unless_draining("box-tool", id, WAIT, out, drain, move || {
                answer(Moor::from_env(&start).map(|moor| {
                    moor.granted(grant)
                        .start_box_run(user, exec, move || stopping.begun())
                }))
            })
        }
        Request::BoxTool(BoxToolReq {
            call: BoxCall::Wait { run },
            ..
        }) => wait_unless_draining("box-tool", id, WAIT, out, drain, move || answer(Ok(run))),
        request @ Request::Close(_) => {
            respond_unless_draining(env, acting, id, request, out, peer).map(Moor::for_place)
        }
        request => respond_unless_draining(env, acting, id, request, out, peer),
    }
}
