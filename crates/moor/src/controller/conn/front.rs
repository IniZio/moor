//! The requests only a front sends, which carry no identity to authorize.

use moor_core::user::Caller;
use moor_core::{Request, Response};

use super::Peer;
use crate::env::Env;
use crate::Moor;

/// Answers `request` when it is one only a front may send, and which a front sends with no
/// identity to authorize: the webhook outbox, `step-report`, `machine-pair` and `github-hook`. None for
/// every other request, and for these when the runner sends them, which the usual authorization
/// then refuses.
pub(super) fn front_request(env: &Env, peer: &Peer, request: &Request) -> Option<Response> {
    let outbox = matches!(
        request,
        Request::WebhookClaim(_) | Request::WebhookReport(_)
    );
    let pair = matches!(request, Request::MachinePair(_) | Request::GitHubHook(_));
    if matches!(peer.caller, Ok(Caller::Runner))
        || !(outbox || pair || matches!(request, Request::StepReport(_)))
    {
        return None;
    }
    Some(match (Moor::from_env(env), request) {
        (Ok(moor), Request::MachinePair(req)) => moor.call_machine_pair(&peer.caller, req),
        (Ok(moor), Request::GitHubHook(req)) => moor.call_github_hook(&peer.caller, req),
        (Ok(moor), _) if outbox => moor.call_webhook_outbox(&peer.caller, &peer.vault, request),
        (Ok(moor), _) => moor.call_step_report(&peer.caller, request),
        (Err(e), _) => e.into_response(request.cmd(), vec![]),
    })
}
