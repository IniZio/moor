//! `/moor machine add|list|remove`: a person's own machines, reached over the host's ssh. `add`
//! opens a modal for the name, ssh target and labels, and sends `machine-add` as that Slack user.
//! Nothing secret is stored here. These are setup commands; nothing reads words in a thread.

use moor_core::request::{AsReq, MachineAddReq, MachineRemoveReq, Request};
use serde_json::{json, Value};

use crate::credentials::SlackUser;
use crate::db::Db;
use crate::login::vault::{send, Call};
use crate::mcp::{field, name_ok, text_ack, BAD_ID};
use crate::socket::{AfterAck, Kept, Slash, TokenSubmission};
use crate::worker::Job;

/// `callback_id` of the add modal.
pub(crate) const MACHINE_CALLBACK: &str = "moor_machine";

/// The add modal's `private_metadata`, which tells a submission from a login one.
pub(crate) const MACHINE_FORM: &str = "machine";

/// Shown above the inputs.
const HELP: &str = "Example: name minion, ssh target moor@minion. The host's own ssh key \
    reaches the machine; nothing secret is stored.";

/// The reply to a `/moor machine` that is not one of the three.
const USAGE: &str = "Usage: `/moor machine add` opens a form for a machine of yours, \
    `/moor machine list` shows your machines and `/moor machine remove <name>` drops one.";

/// The modal `/moor machine add` opens.
pub(crate) fn form() -> Value {
    let input = |block: &str, label: &str, hint: Option<&str>| {
        let mut input = json!({
            "type": "input",
            "block_id": block,
            "optional": block == "labels",
            "label": {"type": "plain_text", "text": label},
            "element": {"type": "plain_text_input", "action_id": block},
        });
        if let (Some(hint), Some(map)) = (hint, input.as_object_mut()) {
            map.insert("hint".into(), json!({"type": "plain_text", "text": hint}));
        }
        input
    };
    json!({
        "type": "modal",
        "callback_id": MACHINE_CALLBACK,
        "private_metadata": MACHINE_FORM,
        "title": {"type": "plain_text", "text": "moor machine"},
        "submit": {"type": "plain_text", "text": "Add"},
        "close": {"type": "plain_text", "text": "Cancel"},
        "blocks": [
            {"type": "context", "elements": [{"type": "plain_text", "text": HELP}]},
            input("name", "Name", None),
            input("ssh", "SSH target", Some("user@host or user@host:port")),
            input("labels", "Labels", Some("Comma-separated, optional.")),
        ],
    })
}

/// A modal that shows `text` as plain text.
fn notice(text: &str) -> Value {
    json!({
        "type": "modal",
        "callback_id": MACHINE_CALLBACK,
        "title": {"type": "plain_text", "text": "moor machine"},
        "close": {"type": "plain_text", "text": "Close"},
        "blocks": [{"type": "section", "text": {"type": "plain_text", "text": text}}],
    })
}

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

/// `/moor machine ...`: `add` opens the modal after an empty ack, `list` and `remove` queue a
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
        (Some("add"), None, _) => {
            let then = slash
                .trigger_id
                .clone()
                .filter(|_| first)
                .map(|trigger_id| AfterAck::Machine { trigger_id });
            (Kept { ack: None, then }, None)
        }
        (Some("list"), None, _) => (text_ack("Looking up your machines…"), Some(job(None))),
        (Some("remove"), Some(name), None) if name_ok(name) => (
            text_ack(&format!("Removing `{name}`…")),
            Some(job(Some(name))),
        ),
        _ => (text_ack(USAGE), None),
    };
    Ok((kept, job.filter(|_| first)))
}

/// The labels typed in a comma-separated field.
fn labels(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Handles a submitted add modal, as the Slack user who submitted it: enrolls them when new,
/// then sends `machine-add`. A refusal is shown to them in the controller's own words.
pub(crate) fn submission(sub: &TokenSubmission, call: &mut Call<'_>) -> Value {
    let update = |text: &str| json!({"response_action": "update", "view": notice(text)});
    if SlackUser::parse(&sub.user).is_none() {
        return update(&format!("{BAD_ID} Nothing was stored."));
    }
    let (name, ssh) = (field(sub, "name"), field(sub, "ssh"));
    let add = MachineAddReq {
        identity: None,
        name: name.clone(),
        ssh,
        labels: labels(&field(sub, "labels")),
    };
    let enroll = Request::Enroll(AsReq { identity: None });
    if let Err(why) = send(call, &sub.user, enroll) {
        return update(&format!("Could not set you up: {why}. Nothing was stored."));
    }
    match send(call, &sub.user, Request::MachineAdd(add)) {
        Ok(_) => update(&format!("Added `{name}`.")),
        Err(why) => update(&why),
    }
}

/// The body of `/moor machine list` for `user`: each machine's name, ssh target and labels, or
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
        return "You have no machines. Add one with `/moor machine add`.".to_owned();
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
            format!("{} {} ({labels})", text(m, "name"), text(m, "ssh"))
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
