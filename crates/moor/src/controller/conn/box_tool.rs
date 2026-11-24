//! The `box-tool` request: a claimed agent place's `exec_in_box` or `start_box`, run as the
//! place's user once [`Moor::admit_box_tool`] allows it.

use moor_core::request::{BoxCall, BoxToolReq};
use moor_core::user::Identity;
use moor_core::{Request, Response};

use super::{piped, respond_unless_draining, Acting, Duplex, Peer};
use crate::env::Env;
use crate::Moor;

/// Answers `req`: refused, the command it runs streamed over `link`, or the start it makes.
/// None when the drain began before a start's answer.
pub(super) fn box_tool(
    env: &Env,
    peer: &Peer,
    id: &str,
    asserted: Option<&Identity>,
    req: &BoxToolReq,
    (input, out): Duplex<'_>,
) -> Option<Response> {
    let admitted = Moor::from_env(env).and_then(|m| m.admit_box_tool(&peer.caller, asserted, req));
    match admitted {
        Err(e) => Some(e.into_response("box-tool", vec![])),
        Ok((grant, _, request @ Request::Exec(_))) => {
            Some(piped(env, peer, grant, id, &request, (input, out)))
        }
        Ok((grant, who, request)) => {
            let acting = Acting {
                who: Some(who),
                grant,
            };
            respond_unless_draining(env, acting, id, request, out, peer)
        }
    }
}

/// Whether `request` streams a command, which takes over the connection.
pub(super) const fn streams(request: &Request) -> bool {
    matches!(
        request,
        Request::BoxTool(BoxToolReq {
            call: BoxCall::Exec { .. },
            ..
        })
    )
}
