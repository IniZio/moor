//! Follows the Box of a `start` cut by a controller restart, reading it by name until it started.

use std::time::Duration;

use moor_core::request::ActAsReq;
use moor_core::{BoxTarget, ErrorKind, Request, Response, ShowReq, StartReq};
use serde_json::Value;

use super::{call, failure_response, ClientError, Target};
use crate::env::Env;
use crate::error::CommandError;

/// Pause between two reads of the Box.
const POLL: Duration = Duration::from_secs(2);

/// Events read with the Box, enough to find why its setup failed.
const EVENTS: u32 = 40;

/// The answer to a `start` that `cut` ended, read once its Box is past setup and its agent launched.
/// `None` when `request` is no start, `cut` is no cut, or no Box of that name can be read.
pub(super) fn start(
    env: &Env,
    target: &Target,
    request: &Request,
    cut: &ClientError,
    mut progress: impl FnMut(&str),
) -> Option<Response> {
    let ClientError::Cut(why) = cut else {
        return None;
    };
    let started = start_of(request)?;
    let name = started.name.as_str();
    let show = sent_as(
        request,
        Request::Show(ShowReq {
            target: Some(BoxTarget::Name(started.name.clone())),
            events: Some(EVENTS),
            agent: false,
            reports: false,
            artifacts: false,
            pane: false,
            transcript: false,
            everyone: false,
        }),
    );
    let mut told = false;
    loop {
        let read = call(env, target, &show, |_| {}, |_| {});
        let value = match read {
            Ok(resp) if resp.error.is_none() => resp.value.unwrap_or_default(),
            Ok(_) | Err(_) if !told => return None,
            Ok(resp) => {
                return Some(Response {
                    cmd: "start".to_owned(),
                    ..resp
                })
            }
            Err(e) => return Some(failure_response("start", &e)),
        };
        if !told {
            told = true;
            progress(&format!("{why}; following {name} on the next controller"));
        }
        let lifecycle = value
            .pointer("/box/lifecycle")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let launched = value
            .pointer("/box/agent/launched")
            .and_then(Value::as_bool);
        match lifecycle {
            "checking" | "setting_up" => {}
            "ready" if !started.no_agent && launched != Some(true) => {}
            "ready" | "pending" | "asleep" => {
                return Some(Response::ok("start", Some(value)));
            }
            _ => return Some(ended(name, lifecycle, &value)),
        }
        std::thread::sleep(POLL);
    }
}

/// The failure for a Box whose start ended `lifecycle`, with why its setup failed when it did.
fn ended(name: &str, lifecycle: &str, value: &Value) -> Response {
    let why = value
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .rev()
        .find(|e| e.get("kind").and_then(Value::as_str) == Some("setup_failed"))
        .and_then(|e| e.pointer("/payload/why"))
        .and_then(Value::as_str)
        .map_or_else(String::new, |why| format!(": {why}"));
    CommandError::new(ErrorKind::Failed, format!("{name} is {lifecycle}{why}"))
        .with_hint(format!("see it with: moor show {name}"))
        .into_response("start", Vec::new())
}

/// The start `request` carries, under any `as` around it.
fn start_of(request: &Request) -> Option<&StartReq> {
    match request {
        Request::Start(start) => Some(start),
        Request::As(act) => start_of(&act.request),
        _ => None,
    }
}

/// `inner` under the same `as` identities as `request`.
fn sent_as(request: &Request, inner: Request) -> Request {
    match request {
        Request::As(act) => Request::As(ActAsReq {
            identity: act.identity.clone(),
            request: Box::new(sent_as(&act.request, inner)),
        }),
        _ => inner,
    }
}
