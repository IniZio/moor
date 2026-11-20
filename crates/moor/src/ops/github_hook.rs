//! `github-hook`: a delivery GitHub posted to the gate's `/hooks/github`.
//!
//! The signature is checked first, against the App's webhook secret, before the event name or
//! the body is read. A delivery that fails it records nothing. One that passes is answered at
//! once, and its pull request event is handled on a thread of its own, since GitHub gives a
//! delivery 10 seconds. Every event but `pull_request` is answered and dropped.

use std::thread;

use moor_core::request::{GitHubHookReq, GitHubHookValue};
use moor_core::user::Caller;
use moor_core::ErrorKind;

use crate::adapters::github::hook::HookSecret;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::stale::ControllerLog;
use crate::ops::workflow::{self, PullEvent};

mod payload;

/// The refusal for any caller but a front.
pub(crate) const FRONT_ONLY: &str = "github-hook serves only a front";

/// Checks the delivery `req` from `caller`, which must be a front, and hands its event to a
/// thread of its own.
pub(crate) fn run(
    cx: &Cx,
    caller: &Result<Caller, String>,
    req: &GitHubHookReq,
) -> Result<serde_json::Value, CommandError> {
    let handle = |event: PullEvent| {
        let owner = cx.detached()?;
        let delivery = req.delivery.clone().unwrap_or_default();
        thread::spawn(move || {
            crate::cx::owners::checked(|| handle(&owner, &delivery, &event));
        });
        Ok(())
    };
    super::to_value(&check(cx, caller, req, &handle)?)
}

/// Like [`run`], handing a checked pull request event to `handle`.
pub(super) fn check(
    cx: &Cx,
    caller: &Result<Caller, String>,
    req: &GitHubHookReq,
    handle: &dyn Fn(PullEvent) -> Result<(), CommandError>,
) -> Result<GitHubHookValue, CommandError> {
    if !matches!(caller, Ok(Caller::Front(_))) {
        return Err(CommandError::new(ErrorKind::Refused, FRONT_ONLY));
    }
    let Some(secret) = cx.env().shared::<HookSecret>() else {
        return Ok(GitHubHookValue::Off);
    };
    if !secret.signed(req.body.as_bytes(), req.signature.as_deref()) {
        return Ok(GitHubHookValue::BadSignature);
    }
    let event = payload::parse(&req.event, req.body.as_bytes())
        .map_err(|why| CommandError::new(ErrorKind::Usage, why))?;
    if let Some(event) = event {
        handle(event)?;
    }
    Ok(GitHubHookValue::Accepted)
}

/// Handles `event` from delivery `delivery` and logs what came of it.
fn handle(cx: &Cx, delivery: &str, event: &PullEvent) {
    let start = |job| workflow::spawn_start(cx, job);
    let lines = match cx.store() {
        Ok(store) => workflow::on_pull_request(cx, store, event, &start),
        Err(e) => vec![e.error.to_string()],
    };
    if let Some(log) = cx.env().shared::<ControllerLog>() {
        for line in lines {
            log.line(&format!("github delivery {delivery}: {line}"));
        }
    }
}
