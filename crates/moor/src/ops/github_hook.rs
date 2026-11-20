//! `github-hook`: a delivery GitHub posted to the gate's `/hooks/github`.
//!
//! The signature is checked first, against the App's webhook secret, before the event name or
//! the body is read. A delivery that fails it records nothing. One that passes is answered at
//! once, and its pull request event is handled on a thread of its own, since GitHub gives a
//! delivery 10 seconds. Every event but `pull_request` is answered and dropped.
//!
//! Each delivery leaves one line in the controller's log, built from its [`Outcome`]: refused
//! with the reason, ignored with its event and action, or accepted with what it led to. The
//! secret, the signature and the body are never written.

use std::thread;

use moor_core::request::{GitHubHookReq, GitHubHookValue};
use moor_core::user::Caller;
use moor_core::ErrorKind;

use crate::adapters::github::hook::HookSecret;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::stale::ControllerLog;
use crate::ops::workflow;

mod outcome;
mod payload;

pub(crate) use outcome::{Outcome, Refusal};
pub(crate) use payload::Pulled;

/// The refusal for any caller but a front.
pub(crate) const FRONT_ONLY: &str = "github-hook serves only a front";

/// Checks the delivery `req` from `caller`, which must be a front, and hands its event to a
/// thread of its own.
pub(crate) fn run(
    cx: &Cx,
    caller: &Result<Caller, String>,
    req: &GitHubHookReq,
) -> Result<serde_json::Value, CommandError> {
    let handle = |pulled: Pulled| {
        let owner = cx.detached()?;
        let delivery = req.delivery.clone();
        thread::spawn(move || {
            crate::cx::owners::checked(|| handle(&owner, delivery.as_deref(), &pulled));
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
    handle: &dyn Fn(Pulled) -> Result<(), CommandError>,
) -> Result<GitHubHookValue, CommandError> {
    if !matches!(caller, Ok(Caller::Front(_))) {
        return Err(CommandError::new(ErrorKind::Refused, FRONT_ONLY));
    }
    let outcome = judge(cx, req);
    if let Some(line) = outcome.line(&req.event) {
        say(cx, req.delivery.as_deref(), &line);
    }
    match outcome {
        Outcome::Accepted(pulled) => {
            handle(pulled)?;
            Ok(GitHubHookValue::Accepted)
        }
        Outcome::Ignored { .. } => Ok(GitHubHookValue::Accepted),
        Outcome::Refused(Refusal::NoSecret) => Ok(GitHubHookValue::Off),
        Outcome::Refused(Refusal::BadSignature) => Ok(GitHubHookValue::BadSignature),
        Outcome::Refused(Refusal::Unparsable(why)) => Err(CommandError::new(ErrorKind::Usage, why)),
    }
}

/// What `req` is: refused by its signature or body, ignored for its event or action, or a pull
/// request event to handle. Nothing is written or started here.
fn judge(cx: &Cx, req: &GitHubHookReq) -> Outcome {
    let Some(secret) = cx.env().shared::<HookSecret>() else {
        return Outcome::Refused(Refusal::NoSecret);
    };
    if !secret.signed(req.body.as_bytes(), req.signature.as_deref()) {
        return Outcome::Refused(Refusal::BadSignature);
    }
    match payload::parse(&req.event, req.body.as_bytes()) {
        Ok(payload::Parsed::Pull(pulled)) => Outcome::Accepted(pulled),
        Ok(payload::Parsed::Other { action }) => Outcome::Ignored { action },
        Err(why) => Outcome::Refused(Refusal::Unparsable(why)),
    }
}

/// Writes `line` as a line of delivery `delivery` to the controller's log.
fn say(cx: &Cx, delivery: Option<&str>, line: &str) {
    if let Some(log) = cx.env().shared::<ControllerLog>() {
        log.line(&outcome::delivery_line(delivery, line));
    }
}

/// Handles `pulled` from delivery `delivery` and logs its one line with what came of it.
fn handle(cx: &Cx, delivery: Option<&str>, pulled: &Pulled) {
    let start = |job| workflow::spawn_start(cx, job);
    handle_with(cx, delivery, pulled, &start);
}

/// Like [`handle`], starting each claimed run through `start`.
pub(crate) fn handle_with(
    cx: &Cx,
    delivery: Option<&str>,
    pulled: &Pulled,
    start: &dyn Fn(workflow::StartJob) -> Result<(), CommandError>,
) {
    let lines = match cx.store() {
        Ok(store) => workflow::on_pull_request(cx, store, &pulled.event, start),
        Err(e) => vec![e.error.to_string()],
    };
    say(cx, delivery, &outcome::accepted_line(pulled, &lines));
}
