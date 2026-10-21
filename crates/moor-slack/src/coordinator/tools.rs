//! The coordinator's tools. Each acts only on the Boxes `slack.db` records for its thread. A
//! tool that changes a Box works only in a turn that holds a message from the person who
//! asked for that Box, writes a journal row before it acts, and refuses in a rerun to repeat
//! what the cut-off turn's journal holds.

use moor_client::ClientError;
use moor_core::request::{Request, ShowReq, SteerReq};
use moor_core::response::{ExitClass, Response};
use serde_json::Value;

use super::context::{action_line, Action};
use crate::db::{BoxState, Db, OpenTurn, Prompt, SlackBox};

/// One tool call, parsed from its name and arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Call {
    /// `list_boxes`.
    ListBoxes,
    /// `start_box`.
    StartBox {
        /// What the Box should do.
        goal: String,
        /// The repo as given, when given.
        repo: Option<String>,
    },
    /// `steer_box`.
    SteerBox {
        /// The Box's name as given.
        box_name: String,
        /// What to send.
        text: String,
    },
    /// `close_box`.
    CloseBox {
        /// The Box's name as given.
        box_name: String,
    },
    /// `answer_box`.
    AnswerBox {
        /// The Box's name as given.
        box_name: String,
        /// The id of the prompt to answer.
        prompt: i64,
        /// The option to pick, numbered from 1.
        choice: u32,
    },
}

impl Call {
    /// The call `name` with `args`, or why it is not one.
    pub(crate) fn parse(name: &str, args: &Value) -> Result<Self, String> {
        let text = |key: &str| {
            args.get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("{name} needs a non-empty `{key}`."))
        };
        match name {
            "list_boxes" => Ok(Self::ListBoxes),
            "start_box" => Ok(Self::StartBox {
                goal: text("goal")?,
                repo: text("repo").ok(),
            }),
            "steer_box" => Ok(Self::SteerBox {
                box_name: text("box")?,
                text: text("text")?,
            }),
            "close_box" => Ok(Self::CloseBox {
                box_name: text("box")?,
            }),
            "answer_box" => Ok(Self::AnswerBox {
                box_name: text("box")?,
                prompt: number(name, args, "prompt")?,
                choice: number(name, args, "choice")?,
            }),
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
    /// The current time in unix seconds.
    pub(crate) clock: fn() -> i64,
    /// Most Boxes the thread has open at once.
    pub(crate) max_open_boxes: u32,
}

/// Runs `call` in `thread`.
pub(crate) fn run(scope: &mut Scope<'_>, call: &Call) -> Reply {
    let ran = match call {
        Call::ListBoxes => list(scope),
        Call::StartBox { goal, repo } => start(scope, goal, repo.as_deref()),
        Call::SteerBox { box_name, text } => steer(scope, box_name, text),
        Call::CloseBox { box_name } => close(scope, box_name),
        Call::AnswerBox {
            box_name,
            prompt,
            choice,
        } => answer(scope, box_name, *prompt, *choice),
    };
    ran.unwrap_or_else(|e| Reply::refused(format!("The front could not read slack.db: {e}")))
}

/// `list_boxes`.
fn list(scope: &mut Scope<'_>) -> rusqlite::Result<Reply> {
    let boxes = scope.db.thread_boxes(scope.channel, scope.thread_ts)?;
    if boxes.is_empty() {
        return Ok(Reply::done("This thread has no Box."));
    }
    let prompts = scope.db.open_prompts(scope.channel, scope.thread_ts)?;
    Ok(Reply::done(
        box_lines(&boxes, &prompts, scope.call).join("\n"),
    ))
}

/// One entry per Box: its name, repo, requester and state, with the controller's view of a
/// Box that runs, then each of its `prompts` that is open.
pub(crate) fn box_lines(
    boxes: &[SlackBox],
    prompts: &[Prompt],
    call: Controller<'_>,
) -> Vec<String> {
    boxes
        .iter()
        .map(|b| {
            let state = match &b.state {
                BoxState::Starting => "starting".to_owned(),
                BoxState::Bound { .. } => shown(b, call),
                BoxState::Closing { .. } => "closing".to_owned(),
                BoxState::Gone { .. } => "closed".to_owned(),
            };
            let mut entry = format!(
                "- Box `{}` on {}, asked for by <@{}>: {state}",
                b.box_name.as_str(),
                b.repo,
                b.requested_by
            );
            for p in prompts.iter().filter(|p| p.box_name == b.box_name) {
                entry.push_str(&prompt_lines(p));
            }
            entry
        })
        .collect()
}

/// The open prompt `p` under its Box's line: its id, then its question and options in a Box
/// quote, each line of which starts with `> `.
fn prompt_lines(p: &Prompt) -> String {
    let mut quoted = String::new();
    let mut quote = |text: &str| {
        for line in text.lines() {
            quoted.push_str("\n  > ");
            quoted.push_str(line);
        }
    };
    quote(&p.question);
    for (i, option) in p.options.iter().enumerate() {
        quote(&format!("{}. {option}", i + 1));
    }
    format!(
        "\n  Open prompt {}, answered with answer_box. In a Box quote:{quoted}",
        p.id
    )
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

/// `steer_box`: the journal row, then `steer`, then its outcome.
fn steer(scope: &mut Scope<'_>, box_name: &str, text: &str) -> rusqlite::Result<Reply> {
    let target = match changeable(scope, "steer_box", box_name, text)? {
        Ok(target) => target,
        Err(refusal) => return Ok(refusal),
    };
    let name = target.boxed.box_name.as_str().to_owned();
    if let Some(refusal) = not_running(&target.boxed, "steer") {
        return Ok(refusal);
    }
    let seq = scope
        .db
        .begin_action(target.turn.id, &target.action, (scope.clock)())?;
    let request = Request::Steer(SteerReq {
        target: target.boxed.box_name.clone().into(),
        text: Some(text.to_owned()),
        menu: None,
    });
    let (outcome, reply) = match (scope.call)(request) {
        Ok(r) if r.class == ExitClass::Ok => (
            format!("sent to Box `{name}`"),
            Reply::done(format!("Sent to Box `{name}`.")),
        ),
        Ok(r) => {
            let why = error_text(r);
            let reply = Reply::refused(format!("Not sent: {why}"));
            (format!("the controller refused: {why}"), reply)
        }
        Err(e) => {
            let said = format!("the controller did not answer: {e}");
            let reply = Reply::refused(format!("Maybe not sent: {said}"));
            (said, reply)
        }
    };
    scope
        .db
        .end_action(target.turn.id, seq, &outcome, (scope.clock)())?;
    Ok(reply)
}

/// `close_box`: the journal row and the Box marked `closing`, in one transaction. The worker
/// sends the close and posts its result in the thread.
fn close(scope: &Scope<'_>, box_name: &str) -> rusqlite::Result<Reply> {
    let target = match changeable(scope, "close_box", box_name, "")? {
        Ok(target) => target,
        Err(refusal) => return Ok(refusal),
    };
    let name = target.boxed.box_name.as_str();
    if let Some(refusal) = not_running(&target.boxed, "close") {
        return Ok(refusal);
    }
    let outcome = format!("Box `{name}` is closing");
    let db = scope.db;
    let now = (scope.clock)();
    db.atomically(|| {
        let seq = db.begin_action(target.turn.id, &target.action, now)?;
        db.mark_closing(&target.boxed.box_name)?;
        db.end_action(target.turn.id, seq, &outcome, now)
    })?;
    Ok(Reply::done(format!(
        "{outcome}. The thread will say when it has closed."
    )))
}

/// A Box a tool may change in this turn, with the turn and the journal row to write.
struct Target {
    /// The turn that runs now.
    turn: OpenTurn,
    /// The Box.
    boxed: SlackBox,
    /// The journal row for the call.
    action: Action,
}

/// The Box `box_name` names, when `tool` may change it now: a turn runs, the Box is one of
/// this thread's, its requester wrote in this turn, and a rerun's cut-off turn did not
/// already do the same. Otherwise the refusal.
fn changeable(
    scope: &Scope<'_>,
    tool: &str,
    box_name: &str,
    args: &str,
) -> rusqlite::Result<Result<Target, Reply>> {
    let db = scope.db;
    let Some(turn) = db.open_turn(scope.channel, scope.thread_ts)? else {
        return Ok(Err(Reply::refused("No turn of this thread is running.")));
    };
    let wanted = box_name.trim().trim_matches('`');
    let rows = db.thread_boxes(scope.channel, scope.thread_ts)?;
    let Some(boxed) = rows.into_iter().find(|b| b.box_name.as_str() == wanted) else {
        return Ok(Err(Reply::refused(format!(
            "This thread has no Box `{wanted}`. Call list_boxes for its Boxes."
        ))));
    };
    let by = boxed.requested_by.clone();
    if !turn.authors.contains(&by) {
        return Ok(Err(Reply::refused(format!(
            "Only <@{by}> can change Box `{wanted}`, and <@{by}> wrote nothing in this turn."
        ))));
    }
    let action = Action {
        turn: turn.id,
        requested_by: by,
        tool: tool.to_owned(),
        box_name: boxed.box_name.as_str().to_owned(),
        args: args.to_owned(),
        outcome: None,
    };
    if let Some(lost) = turn.rerun_of {
        let done = db.turn_actions(lost)?;
        let same = |a: &&Action| {
            a.tool == action.tool && a.box_name == action.box_name && a.args == action.args
        };
        if let Some(earlier) = done.iter().find(same) {
            return Ok(Err(Reply::refused(format!(
                "Your last turn already did this, so it is not done again. {}",
                action_line(earlier)
            ))));
        }
    }
    Ok(Ok(Target {
        turn,
        boxed,
        action,
    }))
}

/// The refusal of `verb` on a Box that does not run.
fn not_running(boxed: &SlackBox, verb: &str) -> Option<Reply> {
    let name = boxed.box_name.as_str();
    match boxed.state {
        BoxState::Bound { .. } => None,
        BoxState::Starting => Some(Reply::refused(format!(
            "Box `{name}` is still starting. Try to {verb} it once it has started."
        ))),
        BoxState::Closing { .. } => Some(Reply::refused(format!("Box `{name}` is closing."))),
        BoxState::Gone { .. } => Some(Reply::refused(format!("Box `{name}` is closed."))),
    }
}

/// The error text of a failed reply.
fn error_text(reply: Response) -> String {
    reply
        .error
        .map_or_else(|| format!("{} failed", reply.cmd), |e| e.message)
}

mod spec;
pub(crate) use spec::{names, SPECS};

mod answer;
use answer::{answer, number};

mod start;
use start::start;

#[cfg(test)]
mod tests;
