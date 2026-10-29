//! What the tools add to the controller's answers so a model can tell a person what a Box is
//! doing, in the words the herdr card uses.

use moor_core::request::{Request as ControllerRequest, ShowReq};
use moor_core::view::progress::{self, Happened, Progress};
use serde_json::{json, Value};

use super::target;

/// What a tool adds to the controller's answer so a model can tell the user what is going on.
#[derive(Clone, Copy)]
pub(super) enum Reply {
    /// Nothing.
    AsIs,
    /// How to follow the start.
    Started,
    /// One Box's status, failure and steps.
    Detailed,
    /// Each Box's status.
    Listed,
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
            Self::Started => started(value),
            Self::Detailed => detailed(value),
            Self::Listed => listed(value, ask),
        }
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
            "checking" | "waiting_for_box" | "setting_up" => Self::Starting,
            _ => Self::Settled,
        }
    }
}

/// The one line a person reads about a Box in `lifecycle` whose latest events are `events`.
fn status(lifecycle: &str, events: &[Happened]) -> String {
    match Stage::of(lifecycle) {
        Stage::Failed => progress::failure(events).map_or_else(
            || lifecycle.replace('_', " "),
            |why| format!("failed: {}", why.replace('\n', " ")),
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

/// Adds one Box's status, failure and steps to `show`'s answer, then trims its raw events.
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
    reply.insert("status".to_owned(), json!(status(&lifecycle, &events)));
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
        if let Some(row) = row.as_object_mut() {
            row.insert("status".to_owned(), json!(status(&lifecycle, &events)));
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
