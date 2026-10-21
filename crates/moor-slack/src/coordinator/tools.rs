//! The coordinator's tools. Each acts only on the Boxes `slack.db` records for its thread.

use moor_client::ClientError;
use moor_core::request::{Request, ShowReq};
use moor_core::response::{ExitClass, Response};
use serde_json::{json, Value};

use crate::db::{BoxState, Db, SlackBox};

/// What a tool does, for the agent's tool list.
pub(crate) struct Spec {
    /// The tool's name.
    pub(crate) name: &'static str,
    /// What it does.
    pub(crate) description: &'static str,
    /// Its parameters as a JSON schema, each with a one-line description.
    pub(crate) input: fn() -> Value,
}

/// Every tool the coordinator gets.
pub(crate) const SPECS: [Spec; 1] = [Spec {
    name: "list_boxes",
    description: "List this thread's Boxes: name, repo, requester and state as the controller \
                  reports it now.",
    input: || json!({"type": "object", "properties": {}, "additionalProperties": false}),
}];

/// The names of every tool the coordinator gets.
pub(crate) fn names() -> Vec<&'static str> {
    SPECS.iter().map(|s| s.name).collect()
}

/// One tool call, parsed from its name and arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Call {
    /// `list_boxes`.
    ListBoxes,
}

impl Call {
    /// The call `name`, or why it is not one.
    pub(crate) fn parse(name: &str, _args: &Value) -> Result<Self, String> {
        match name {
            "list_boxes" => Ok(Self::ListBoxes),
            other => Err(format!("There is no tool `{other}`.")),
        }
    }
}

/// What a tool says back: text for the model, and whether the call was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reply {
    /// The text.
    pub(crate) text: String,
    /// True when the call did not do what it asked.
    pub(crate) is_error: bool,
}

impl Reply {
    /// A call that did what it asked.
    fn done(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    /// A call that was refused or failed.
    fn refused(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// Sends one request to the controller.
pub(crate) type Controller<'a> = &'a mut dyn FnMut(Request) -> Result<Response, ClientError>;

/// Where the tools act: one thread, its rows in `slack.db`, and the controller.
pub(crate) struct Scope<'a> {
    /// `slack.db`.
    pub(crate) db: &'a Db,
    /// Channel id of the thread.
    pub(crate) channel: &'a str,
    /// Timestamp of the thread's root message.
    pub(crate) thread_ts: &'a str,
    /// The controller.
    pub(crate) call: Controller<'a>,
}

/// Runs `call` in `scope`.
pub(crate) fn run(scope: &mut Scope<'_>, call: &Call) -> Reply {
    let ran = match call {
        Call::ListBoxes => list(scope),
    };
    ran.unwrap_or_else(|e| Reply::refused(format!("The front could not read slack.db: {e}")))
}

/// `list_boxes`.
fn list(scope: &mut Scope<'_>) -> rusqlite::Result<Reply> {
    let boxes = scope.db.thread_boxes(scope.channel, scope.thread_ts)?;
    if boxes.is_empty() {
        return Ok(Reply::done("This thread has no Box."));
    }
    Ok(Reply::done(box_lines(&boxes, scope.call).join("\n")))
}

/// One line per Box: its name, repo, requester and state, with the controller's view of a
/// Box that runs.
pub(crate) fn box_lines(boxes: &[SlackBox], call: Controller<'_>) -> Vec<String> {
    boxes
        .iter()
        .map(|b| {
            let state = match &b.state {
                BoxState::Starting => "starting".to_owned(),
                BoxState::Bound { .. } => shown(b, call),
                BoxState::Closing { .. } => "closing".to_owned(),
                BoxState::Gone { .. } => "closed".to_owned(),
            };
            format!(
                "- Box `{}` on {}, asked for by <@{}>: {state}",
                b.box_name.as_str(),
                b.repo,
                b.requested_by
            )
        })
        .collect()
}

/// A running Box's state as `show` reports it.
fn shown(b: &SlackBox, call: Controller<'_>) -> String {
    let request = Request::Show(ShowReq {
        target: Some(b.box_name.clone().into()),
        events: None,
        agent: true,
        reports: false,
        artifacts: false,
        pane: false,
    });
    let view = match call(request) {
        Ok(reply) if reply.class == ExitClass::Ok => reply.value.unwrap_or(Value::Null),
        Ok(reply) => return format!("the controller did not say: {}", error_text(reply)),
        Err(e) => return format!("the controller is not answering: {e}"),
    };
    let field = |path: &[&str]| {
        let found = path.iter().try_fold(&view, |v, key| v.get(key));
        found
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned()
    };
    format!(
        "lifecycle {}, agent {}, branch {}",
        field(&["box", "lifecycle"]),
        field(&["box", "agent", "state"]),
        field(&["box", "branch"])
    )
}

/// The error text of a failed reply.
fn error_text(reply: Response) -> String {
    reply
        .error
        .map_or_else(|| format!("{} failed", reply.cmd), |e| e.message)
}

#[cfg(test)]
mod tests;
