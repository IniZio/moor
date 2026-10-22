//! Each Box's status message: the view folded from its events, the text it renders, and the
//! planner that decides which messages to post or edit now.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use moor_core::name::BoxId;
use serde_json::Value;

use crate::db::{BoxState as RowState, FedBox};
use crate::escape::escape;

/// Least time between two sends for one Box; changes in between coalesce.
pub(super) const UPDATE_EVERY: Duration = Duration::from_secs(2);

/// Longest question shown, in characters.
const QUESTION_MAX: usize = 300;

/// Where the Box is in its life.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) enum State {
    /// Created or setting up; not yet working.
    #[default]
    Starting,
    /// The agent is working.
    Working,
    /// The agent is quiet.
    Idle,
    /// The Box is asleep.
    Asleep,
    /// The agent asked a question.
    NeedsInput(String),
    /// The Box closed.
    Closed,
    /// Admission refused the Box.
    Rejected(String),
    /// Box setup failed.
    SetupFailed(String),
}

/// What is known about the Box's machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Machine {
    /// Requested, not yet ready.
    Requested,
    /// Ready for use.
    Ready,
    /// Acquiring failed.
    Failed,
    /// Asleep.
    Asleep,
    /// The driver lost it.
    Gone,
    /// Deleted.
    Deleted,
}

/// The status of one Box, folded from its events.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct View {
    /// Box state.
    pub(super) state: State,
    /// Machine state, once any machine event arrived.
    pub(super) machine: Option<Machine>,
}

impl View {
    /// Applies one event of `kind` with `payload`; unrelated kinds change nothing.
    pub(super) fn fold(&mut self, kind: &str, payload: &Value) {
        let text = |key: &str| {
            payload
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        match kind {
            "started" | "setting_up" => self.state = State::Starting,
            "working" | "busy" | "woke" => self.state = State::Working,
            "idled" => self.state = State::Idle,
            "asleep" => self.state = State::Asleep,
            "needs_input" => self.state = State::NeedsInput(text("question")),
            "closed" => self.state = State::Closed,
            "rejected" => self.state = State::Rejected(text("reason")),
            "setup_failed" => self.state = State::SetupFailed(text("line")),
            "box_requested" => self.machine = Some(Machine::Requested),
            "box_ready" => self.machine = Some(Machine::Ready),
            "box_failed" => self.machine = Some(Machine::Failed),
            "box_asleep" => self.machine = Some(Machine::Asleep),
            "box_gone" => self.machine = Some(Machine::Gone),
            "box_deleted" => self.machine = Some(Machine::Deleted),
            _ => {}
        }
    }
}

/// The status message of Box `name`.
pub(super) fn render(name: &str, view: &View) -> String {
    let state = match &view.state {
        State::Starting => "starting".to_owned(),
        State::Working => "working".to_owned(),
        State::Idle => "idle".to_owned(),
        State::Asleep => "asleep".to_owned(),
        State::Closed => "closed".to_owned(),
        State::NeedsInput(q) if q.is_empty() => "needs input".to_owned(),
        State::NeedsInput(q) => {
            let short: String = q.chars().take(QUESTION_MAX).collect();
            let dots = if short.len() < q.len() { "..." } else { "" };
            format!("needs input: {}{dots}", escape(&short))
        }
        State::Rejected(r) => format!("rejected: {}", escape(r)),
        State::SetupFailed(l) => format!("setup failed: {}", escape(l)),
    };
    let mut text = format!("*{}*: {state}", escape(name));
    if let Some(m) = view.machine {
        let m = match m {
            Machine::Requested => "requested",
            Machine::Ready => "ready",
            Machine::Failed => "failed",
            Machine::Asleep => "asleep",
            Machine::Gone => "gone",
            Machine::Deleted => "deleted",
        };
        let _ = write!(text, "\nBox: {m}");
    }
    text
}

/// One status send the planner wants made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Step {
    /// The Box the message belongs to.
    pub(super) box_name: String,
    /// Channel of the Box's thread.
    pub(super) channel: String,
    /// Timestamp of the thread's root message.
    pub(super) thread_ts: String,
    /// The message to edit; none means post a new one.
    pub(super) ts: Option<String>,
    /// Text to send.
    pub(super) text: String,
}

/// Decides which Boxes need a status send now, honoring the per-Box rate limit.
#[derive(Default)]
pub(super) struct Planner {
    /// When a send was last tried, by Box name.
    tried: HashMap<String, Instant>,
}

impl Planner {
    /// Sends due at `now`: a post for a Box with no status message yet, an edit for a Box
    /// whose text changed. A Box that is gone gets its final text in the message it has, and
    /// a gone Box with no message gets none.
    pub(super) fn plan(
        &self,
        boxes: &[FedBox],
        views: &HashMap<BoxId, View>,
        now: Instant,
    ) -> Vec<Step> {
        let mut steps = Vec::new();
        for fed in boxes {
            let b = &fed.boxed;
            let (box_id, gone) = match &b.state {
                RowState::Starting => continue,
                RowState::Bound { box_id, .. } | RowState::Closing { box_id, .. } => {
                    (*box_id, false)
                }
                RowState::Gone { box_id, .. } => (*box_id, true),
            };
            if gone && fed.status_ts.is_none() {
                continue;
            }
            let mut view = views.get(&box_id).cloned().unwrap_or_default();
            if gone {
                view.state = State::Closed;
            }
            let name = b.box_name.as_str();
            let text = render(name, &view);
            if fed.status_ts.is_some() && fed.status_text.as_ref() == Some(&text) {
                continue;
            }
            if self
                .tried
                .get(name)
                .is_some_and(|at| now.duration_since(*at) < UPDATE_EVERY)
            {
                continue;
            }
            steps.push(Step {
                box_name: name.to_owned(),
                channel: b.channel.clone(),
                thread_ts: b.thread_ts.clone(),
                ts: fed.status_ts.clone(),
                text,
            });
        }
        steps
    }

    /// Notes a send of `step` tried at `now`; the next send for its Box waits out the window.
    pub(super) fn record(&mut self, step: &Step, now: Instant) {
        self.tried.insert(step.box_name.clone(), now);
    }
}

#[cfg(test)]
mod tests;
