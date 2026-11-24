//! The `box-tool` request: a claimed agent place's `exec_in_box`, `exec_wait`, `start_box` or
//! `close_box`, run as the place's user once [`Moor::admit_box_tool`] allows it.
//!
//! A command runs on past its call: the call answers within [`runs::WINDOW`] with the output so
//! far, and `exec_wait` reads on.

use std::sync::Arc;

use moor_core::request::BoxToolReq;
use moor_core::user::Identity;
use moor_core::{Request, Response};

use super::{respond_unless_draining, Acting, Duplex, Peer};
use crate::controller::progress::{wait_unless_draining, WAIT};
use crate::env::Env;
use crate::ops::box_tool::{for_place, runs, Admitted, Plan};
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
    let (grant, Admitted { user, who, plan }) = match admitted {
        Err(e) => return Some(e.into_response("box-tool", vec![])),
        Ok(admitted) => admitted,
    };
    let drain = peer.conn.drain();
    let (env, draining, stopping) = (env.clone(), Arc::clone(drain), Arc::clone(drain));
    let answer = move |moor: Moor, run| {
        let reply = moor.answer_box_run(user, run, runs::WINDOW, &|| draining.begun());
        match reply {
            Ok(view) => Response::ok("box-tool", serde_json::to_value(view).ok()),
            Err(e) => e.into_response("box-tool", vec![]),
        }
    };
    match plan {
        Plan::Exec(exec) => wait_unless_draining("box-tool", id, WAIT, out, drain, move || {
            let started = Moor::from_env(&env).map(|moor| {
                moor.granted(grant)
                    .start_box_run(user, exec, move || stopping.begun())
            });
            match (started, Moor::from_env(&env)) {
                (Ok(run), Ok(moor)) => answer(moor, run),
                (Err(e), _) | (_, Err(e)) => e.into_response("box-tool", vec![]),
            }
        }),
        Plan::Wait(run) => {
            wait_unless_draining(
                "box-tool",
                id,
                WAIT,
                out,
                drain,
                move || match Moor::from_env(&env) {
                    Ok(moor) => answer(moor, run),
                    Err(e) => e.into_response("box-tool", vec![]),
                },
            )
        }
        Plan::Start(start) => {
            let acting = Acting {
                who: Some(who),
                grant,
            };
            respond_unless_draining(&env, acting, id, Request::Start(*start), out, peer)
        }
        Plan::Close(close) => {
            let acting = Acting {
                who: Some(who),
                grant,
            };
            respond_unless_draining(&env, acting, id, Request::Close(close), out, peer)
                .map(for_place)
        }
    }
}
