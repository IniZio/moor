//! What the tools add to the controller's answers so a model can tell a person what a Box is
//! doing, in the words the herdr card uses.

use moor_core::request::{Request as ControllerRequest, ShowReq};
use moor_core::response::{ErrorDetail, Response as ControllerResponse};
use moor_core::user::SecretName;
use moor_core::view::progress::{self, Happened, Progress};
use moor_core::view::stuck::stuck_of;
use moor_core::view::user::StartNeedsValue;
use serde_json::{json, Value};
use std::sync::Arc;

use super::target;

mod webhook;

/// What a tool adds to the controller's answer so a model can tell the user what is going on.
#[derive(Clone)]
pub(super) enum Reply {
    /// Nothing.
    AsIs,
    /// How to follow the start, and where to store missing secrets given the public origin.
    Started(Option<Arc<str>>),
    /// One Box's status, failure and steps.
    Detailed,
    /// Each Box's status.
    Listed,
    /// A new webhook: where its person sets it up, given the gate's public origin.
    WebhookAdded(Option<Arc<str>>),
    /// Each webhook's state and health in words, and setup links given the public origin.
    WebhooksListed(Option<Arc<str>>),
}

impl Reply {
    /// Adds what this tool adds to `value`, asking the controller through `ask` when it must.
    pub(super) fn apply(
        self,
        value: &mut Value,
        ask: &dyn Fn(ControllerRequest) -> Result<Value, String>,
    ) {
        match self {
            Self::AsIs => {}
            Self::Started(_) => started(value),
            Self::Detailed => detailed(value),
            Self::Listed => listed(value, ask),
            Self::WebhookAdded(origin) => webhook::added(value, origin.as_deref()),
            Self::WebhooksListed(origin) => webhook::listed(value, origin.as_deref(), now()),
        }
    }

    /// The controller's refusal `why` in this tool's words: a start missing secrets sends the
    /// person to the secrets page instead of a command only the controller's host can run.
    pub(super) fn refused(&self, why: String, answered: &ControllerResponse) -> String {
        let Self::Started(Some(origin)) = self else {
            return why;
        };
        let detail = answered.error.as_ref().and_then(|e| e.detail.as_ref());
        if !matches!(detail, Some(ErrorDetail::LoginRequired)) {
            return why;
        }
        let needs = answered
            .value
            .clone()
            .and_then(|v| serde_json::from_value::<StartNeedsValue>(v).ok());
        let Some(needs) = needs.filter(|n| !n.missing.is_empty()) else {
            return why;
        };
        let names: Vec<&str> = needs.missing.iter().map(SecretName::as_str).collect();
        format!(
            "missing secrets {}: ask the person to sign in at {origin}{} and store them, \
             then start again",
            names.join(", "),
            crate::login::secrets::PATH
        )
    }
}

/// The raw events `box_status` keeps; the steps carry the rest.
const RAW_EVENTS: usize = 5;

/// Where a Box is, as far as what to tell a person goes.
enum Stage {
    /// It is down and the events say why.
    Failed,
    /// It is still being set up.
    Starting,
    /// Anything else; the lifecycle says it.
    Settled,
}

impl Stage {
    /// The stage of a Box in `lifecycle`.
    fn of(lifecycle: &str) -> Self {
        match lifecycle {
            "setup_failed" | "rejected" | "closed" => Self::Failed,
            "checking" | "pending" | "setting_up" => Self::Starting,
            _ => Self::Settled,
        }
    }
}

/// The one line a person reads about a Box in `lifecycle` whose latest events are `events`.
fn status(lifecycle: &str, events: &[Happened]) -> String {
    match Stage::of(lifecycle) {
        Stage::Failed => progress::failure(events).map_or_else(
            || lifecycle.replace('_', " "),
            |why| {
                let why = why.replace('\n', " ");
                if why
                    .split(':')
                    .next()
                    .is_some_and(|head| head.ends_with("failed"))
                {
                    why
                } else {
                    format!("failed: {why}")
                }
            },
        ),
        Stage::Starting => {
            let elapsed = Progress::of(events, now()).elapsed.map(progress::span);
            [
                Some("starting".to_owned()),
                elapsed,
                progress::latest(events),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
        }
        Stage::Settled => lifecycle.replace('_', " "),
    }
}

/// `status` with what the agent still waits on, when its row says it ended its turn but waits.
fn waiting(status: String, agent: Option<&Value>) -> String {
    let field = |name| agent.and_then(|a| a.get(name));
    if field("state").and_then(Value::as_str) != Some("waiting") {
        return status;
    }
    let on: Vec<&str> = field("waiting_on")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if on.is_empty() {
        format!("{status} · waiting on background work")
    } else {
        format!("{status} · waiting on {}", on.join(", "))
    }
}

/// `value`'s events, oldest first, as the card reads them.
fn happened(value: &Value) -> Vec<Happened> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

/// The Box's lifecycle in a `show` row.
fn lifecycle(row: &Value) -> &str {
    row.get("lifecycle")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Adds how to follow the start to `start`'s answer.
fn started(value: &mut Value) {
    if let Some(reply) = value.as_object_mut() {
        reply.insert(
            "next".to_owned(),
            json!(
                "the machine is still starting; call box_status with this Box's name to \
                   follow it and to see why it failed, if it does"
            ),
        );
    }
}

/// Adds one Box's status, failure, steps and the agent's last message to `show`'s answer, then trims its raw events.
fn detailed(value: &mut Value) {
    let Some(reply) = value.as_object_mut() else {
        return;
    };
    let events = happened(reply.get("events").unwrap_or(&Value::Null));
    let lifecycle = reply
        .get("box")
        .map(lifecycle)
        .unwrap_or_default()
        .to_owned();
    let line = waiting(
        status(&lifecycle, &events),
        reply.get("box").and_then(|b| b.get("agent")),
    );
    let stuck = stuck_of(&events);
    let line = stuck
        .as_ref()
        .map_or_else(|| line.clone(), |s| format!("{line} · stuck: {}", s.kind));
    reply.insert("status".to_owned(), json!(line));
    reply.insert("stuck".to_owned(), json!(stuck));
    let cold = progress::cold_cache(&events)
        .map(|(reason, detail)| json!({"reason": reason, "detail": detail}));
    reply.insert("cache_cold".to_owned(), json!(cold));
    // The trim below may drop the agent's message, so it gets a key of its own.
    if let Some(said) = reply.get("box").and_then(|b| b.get("said")).cloned() {
        reply.insert("agent_said".to_owned(), said);
    }
    if matches!(Stage::of(&lifecycle), Stage::Failed) {
        reply.insert("failure".to_owned(), json!(progress::failure(&events)));
    }
    if !matches!(Stage::of(&lifecycle), Stage::Settled) {
        reply.insert(
            "steps".to_owned(),
            json!(Progress::of(&events, now()).steps),
        );
    }
    if let Some(Value::Array(raw)) = reply.get_mut("events") {
        raw.drain(..raw.len().saturating_sub(RAW_EVENTS));
    }
}

/// Adds each Box's status to `show`'s list, asking for the events of those not settled.
fn listed(value: &mut Value, ask: &dyn Fn(ControllerRequest) -> Result<Value, String>) {
    let Some(Value::Array(rows)) = value.get_mut("boxes") else {
        return;
    };
    for row in rows {
        let lifecycle = lifecycle(row).to_owned();
        let events = match (
            Stage::of(&lifecycle),
            row.get("name").and_then(Value::as_str),
        ) {
            (Stage::Settled, _) | (_, None) => Vec::new(),
            (_, Some(name)) => events_of(name, ask),
        };
        let agent = row.get("agent").cloned();
        if let Some(row) = row.as_object_mut() {
            let line = waiting(status(&lifecycle, &events), agent.as_ref());
            row.insert("status".to_owned(), json!(line));
        }
    }
}

/// The latest events of Box `name`; none when the controller will not say.
fn events_of(
    name: &str,
    ask: &dyn Fn(ControllerRequest) -> Result<Value, String>,
) -> Vec<Happened> {
    let Ok(target) = target(name) else {
        return Vec::new();
    };
    let shown = ask(ControllerRequest::Show(ShowReq {
        target: Some(target),
        events: Some(progress::EVENTS),
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    }));
    shown.map_or_else(
        |_| Vec::new(),
        |v| happened(v.get("events").unwrap_or(&Value::Null)),
    )
}

/// Now, in unix seconds.
#[expect(
    clippy::disallowed_methods,
    reason = "the tools' one clock read; the progress it feeds takes the time as an argument"
)]
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests;
