//! `/moor machine list|remove`: a person's own machines. A machine joins by running
//! `moor-machine-agent enroll` on it, so `add` only says how. Nothing secret is stored here.
//! These are setup commands; nothing reads words in a thread.

use moor_core::request::{AsReq, MachineRemoveReq, Request};
use serde_json::Value;

use crate::credentials::SlackUser;
use crate::db::Db;
use crate::login::vault::{send, Call};
use crate::mcp::{name_ok, text_ack, BAD_ID};
use crate::socket::{Kept, Slash};
use crate::worker::Job;

/// The reply to `/moor machine add`: machines enroll themselves.
const ENROLL: &str = "A machine joins by enrolling itself: run \
    `moor-machine-agent enroll --server <url> --name <name>` on it, then \
    `moor-machine-agent run`.";

/// The reply to a `/moor machine` that is not one of the three.
const USAGE: &str = "Usage: `/moor machine list` shows your machines, \
    `/moor machine remove <name>` drops one and `/moor machine add` says how to enroll one.";

/// A `/moor machine list` or `/moor machine remove` for the worker; no `name` means list.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MachineJob {
    /// The Slack user who ran the command.
    pub(crate) user: String,
    /// The machine to remove, or none to list.
    pub(crate) name: Option<String>,
    /// Where Slack takes the answer to the command.
    pub(crate) response_url: Option<String>,
}

/// `/moor machine ...`: `add` answers how to enroll, `list` and `remove` queue a
/// job whose answer goes to the user alone through the command's `response_url`.
pub(crate) fn slash(slash: &Slash, db: &Db, now: i64) -> rusqlite::Result<(Kept, Option<Job>)> {
    let first = db.atomically(|| db.event_once(&slash.envelope_id, now))?;
    if SlackUser::parse(&slash.user).is_none() {
        return Ok((text_ack(BAD_ID), None));
    }
    let mut words = slash.text.split_whitespace().skip(1);
    let user = slash.user.clone();
    let response_url = slash.response_url.clone();
    let job = |name: Option<&str>| {
        Job::Machine(MachineJob {
            user: user.clone(),
            name: name.map(str::to_owned),
            response_url: response_url.clone(),
        })
    };
    let (kept, job) = match (words.next(), words.next(), words.next()) {
        (Some("add"), None, _) => (text_ack(ENROLL), None),
        (Some("list"), None, _) => (text_ack("Looking up your machines…"), Some(job(None))),
        (Some("remove"), Some(name), None) if name_ok(name) => (
            text_ack(&format!("Removing `{name}`…")),
            Some(job(Some(name))),
        ),
        _ => (text_ack(USAGE), None),
    };
    Ok((kept, job.filter(|_| first)))
}

/// The body of `/moor machine list` for `user`: each machine's name and labels, or
/// why the controller could not say.
pub(crate) fn listing(call: &mut Call<'_>, user: &str) -> String {
    let reply = match send(call, user, Request::MachineList(AsReq { identity: None })) {
        Ok(reply) => reply,
        Err(why) => {
            return format!(
                "Could not list your machines: {}",
                crate::escape::escape(&why)
            )
        }
    };
    let machines = reply.value.as_ref().and_then(|v| v.get("machines"));
    let machines = machines
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if machines.is_empty() {
        return "You have no machines. Enroll one with `moor-machine-agent enroll` on it."
            .to_owned();
    }
    let text = |m: &Value, k: &str| {
        crate::escape::code(m.get(k).and_then(Value::as_str).unwrap_or_default())
    };
    let lines: Vec<String> = machines
        .iter()
        .map(|m| {
            let labels: Vec<&str> = m
                .get("labels")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let labels = if labels.is_empty() {
                "no labels".to_owned()
            } else {
                crate::escape::escape(&labels.join(", "))
            };
            format!("{} ({labels})", text(m, "name"))
        })
        .collect();
    format!("Your machines:\n{}", lines.join("\n"))
}

/// The answer to `/moor machine remove <name>` for `user`.
pub(crate) fn removal(call: &mut Call<'_>, user: &str, name: &str) -> String {
    let remove = MachineRemoveReq {
        identity: None,
        name: name.to_owned(),
    };
    match send(call, user, Request::MachineRemove(remove)) {
        Ok(reply) => {
            let changed = reply.value.as_ref().and_then(|v| v.get("changed"));
            if changed.and_then(Value::as_bool) == Some(true) {
                format!("Removed `{name}`.")
            } else {
                format!("You have no machine named `{name}`.")
            }
        }
        Err(why) => format!("Could not remove `{name}`: {}", crate::escape::escape(&why)),
    }
}
