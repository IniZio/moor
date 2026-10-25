//! Each Box's status message: the view folded from its events, the text it renders, and the
//! planner that decides which messages to post or edit now.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use moor_core::name::BoxId;
use serde_json::{json, Value};

use crate::db::{BoxState as RowState, FedBox};
use crate::escape::escape;
use crate::stop;

/// Least time between two sends for one Box; changes in between coalesce.
pub(super) const UPDATE_EVERY: Duration = Duration::from_secs(2);

/// Longest question shown, in characters.
const QUESTION_MAX: usize = 300;

/// Seconds a working Box may emit no event before its status says it may be stuck.
pub(super) const STALL_AFTER_SECS: i64 = 10 * 60;

/// Seconds a starting, blocked or unanswered Box may wait before its status says it may be stuck.
pub(super) const STUCK_AFTER_SECS: i64 = 3 * 60;

/// Where the Box is in its life.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) enum State {
    /// Created; setup has not begun.
    #[default]
    Starting,
    /// Setting up the Box.
    SettingUp,
    /// The agent is working.
    Working,
    /// The agent is quiet.
    Idle,
    /// The Box is asleep.
    Asleep,
    /// The agent asked a question.
    NeedsInput(String),
    /// The Box is closing.
    Closing,
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
    /// When `state` began, in milliseconds since the epoch; none before any event.
    pub(super) since_ms: Option<i64>,
    /// When the Box last emitted any event, in milliseconds since the epoch.
    pub(super) last_event_ms: Option<i64>,
    /// When the Box began launching its agent, in milliseconds since the epoch, until the agent
    /// is seen alive. Setup, which can legitimately take long, is not part of it.
    pub(super) launching_ms: Option<i64>,
    /// Whether the launch failed or timed out, which keeps `launching_ms` through the quiet
    /// that follows.
    pub(super) gave_up: bool,
    /// The first-run screen the agent stopped at, with when the event came in milliseconds.
    pub(super) blocked: Option<(String, i64)>,
}

impl View {
    /// Applies one event of `kind` with `payload` that happened at `at_ms`; unrelated kinds
    /// change nothing but the time of the last event.
    pub(super) fn fold(&mut self, kind: &str, payload: &Value, at_ms: i64) {
        let before = self.state.clone();
        self.last_event_ms = Some(at_ms);
        self.apply(kind, payload, at_ms);
        if self.state != before || self.since_ms.is_none() {
            self.since_ms = Some(at_ms);
        }
    }

    /// The agent is alive or the Box left the launch, so there is no launch left to wait on.
    fn forget_launch(&mut self) {
        self.launching_ms = None;
        self.gave_up = false;
    }

    /// Sets the state and machine the event of `kind` leads to.
    fn apply(&mut self, kind: &str, payload: &Value, at_ms: i64) {
        let text = |key: &str| {
            payload
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        match kind {
            "started" => {
                self.state = State::Starting;
                self.forget_launch();
                self.blocked = None;
            }
            "session_started" => {
                self.forget_launch();
                self.blocked = None;
            }
            "agent_said" => self.forget_launch(),
            "session_failed" => self.gave_up = true,
            "launch_timed_out" => {
                let after = payload.get("after_secs").and_then(Value::as_i64);
                self.launching_ms = Some(at_ms - after.unwrap_or(0) * 1000);
                self.gave_up = true;
            }
            "agent_prompt_blocked" => self.blocked = Some((text("prompt"), at_ms)),
            "setting_up" => self.state = State::SettingUp,
            "working" => {
                self.launching_ms = Some(at_ms);
                self.state = State::Working;
            }
            "busy" => {
                self.forget_launch();
                self.state = State::Working;
            }
            "woke" => self.state = State::Working,
            "idled" => {
                if !self.gave_up {
                    self.launching_ms = None;
                }
                self.state = State::Idle;
            }
            "asleep" => {
                self.forget_launch();
                self.state = State::Asleep;
            }
            "needs_input" => self.state = State::NeedsInput(text("question")),
            "closed" => {
                self.forget_launch();
                self.state = State::Closed;
            }
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

/// `secs` as whole minutes: `<1m`, `5m`, `1h 05m`. Coarse on purpose, so the text changes
/// once a minute and the edit loop sends no more than that.
fn minutes(secs: i64) -> String {
    let m = secs.max(0) / 60;
    match m {
        0 => "<1m".to_owned(),
        1..=59 => format!("{m}m"),
        _ => format!("{}h {:02}m", m / 60, m % 60),
    }
}

/// The status message of Box `name` at `now` (unix seconds): its phase, how long the Box has
/// been in it, and a hint when a working Box has been silent for [`STALL_AFTER_SECS`].
pub(super) fn render(name: &str, view: &View, now: i64) -> String {
    let since = |from: Option<i64>| from.map_or(0, |ms| now - ms.div_euclid(1000));
    let timed = |phase: &str| format!("{phase} ({})", minutes(since(view.since_ms)));
    let state = match &view.state {
        State::Starting => timed("starting"),
        State::SettingUp => timed("setting up"),
        State::Working => timed("working"),
        State::Idle => timed("idle"),
        State::Asleep => timed("asleep"),
        State::Closing => "closing".to_owned(),
        State::Closed => "closed".to_owned(),
        State::NeedsInput(q) if q.is_empty() => timed("waiting on a question"),
        State::NeedsInput(q) => {
            let short: String = q.chars().take(QUESTION_MAX).collect();
            let dots = if short.len() < q.len() { "..." } else { "" };
            format!(
                "{}: {}{dots}",
                timed("waiting on a question"),
                escape(&short)
            )
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
    let quiet = since(view.last_event_ms);
    if view.state == State::Working && quiet >= STALL_AFTER_SECS {
        let _ = write!(text, "\nNo event for {}. It may be stuck.", minutes(quiet));
    }
    if let Some(hint) = stuck_hint(view, now) {
        let _ = write!(text, "\n{hint}");
    }
    text
}

/// The line for a Box that is starting, blocked on a first-run screen or waiting on an
/// unanswered question for [`STUCK_AFTER_SECS`]: what it is stuck on and how to recover.
fn stuck_hint(view: &View, now: i64) -> Option<String> {
    let age = |ms: i64| now - ms.div_euclid(1000);
    if let Some((prompt, at)) = &view.blocked {
        if age(*at) >= STUCK_AFTER_SECS {
            return Some(format!(
                "Stopped at its {} screen for {}. It may be stuck; ask me to restart the agent.",
                escape(prompt),
                minutes(age(*at))
            ));
        }
    }
    if let State::NeedsInput(_) = view.state {
        let since = age(view.since_ms?);
        return (since >= STUCK_AFTER_SECS).then(|| {
            format!(
                "Waiting on an answer for {}. It may be stuck; reply here with the answer, or ask me to restart the agent.",
                minutes(since)
            )
        });
    }
    let launching = age(view.launching_ms?);
    (launching >= STUCK_AFTER_SECS).then(|| {
        format!(
            "No agent session after {}. It may be stuck starting; ask me to restart the agent.",
            minutes(launching)
        )
    })
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
    /// Block Kit blocks to send with it: the Stop button while the Box works or waits on a
    /// question, an empty list otherwise, which removes the button from a message that had it.
    pub(super) blocks: Value,
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
        unix: i64,
    ) -> Vec<Step> {
        let mut steps = Vec::new();
        for fed in boxes {
            let b = &fed.boxed;
            let (box_id, forced) = match &b.state {
                RowState::Starting => continue,
                RowState::Bound { box_id, .. } => (*box_id, None),
                RowState::Closing { box_id, .. } => (*box_id, Some(State::Closing)),
                RowState::Gone { box_id, .. } => (*box_id, Some(State::Closed)),
            };
            if matches!(forced, Some(State::Closed)) && fed.status_ts.is_none() {
                continue;
            }
            let mut view = views.get(&box_id).cloned().unwrap_or_default();
            if let Some(state) = forced {
                view.state = state;
            }
            let name = b.box_name.as_str();
            let text = render(name, &view, unix);
            let blocks = match view.state {
                State::Working | State::NeedsInput(_) => stop::blocks(name, &text),
                _ => json!([]),
            };
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
                blocks,
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
