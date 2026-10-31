//! The Boxes the controller shows, reduced to what the herdr sidebar and the card draw.

use moor_client::Client;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::session_state::SessionState;
use moor_core::request::Request;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::card::progress::Happened;
use crate::error::Error;

/// The mark that starts every Box workspace label, and how sync tells its workspaces apart.
pub const BOX_MARK: &str = "◈";

/// What the Box's agent is doing while the Box is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// The agent is running a turn.
    Working,
    /// The agent asked a question and waits for an answer.
    Waiting,
    /// The agent finished its turn.
    Idle,
}

/// Where a Box is in its life, as the sidebar shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Still being set up; there is nothing to attach to yet.
    Starting,
    /// Up, with an agent to attach to.
    Live(Activity),
    /// Up, but every slot on its machine is taken.
    WaitingForMachine,
    /// Paused; its files stay.
    Asleep,
    /// Gone for good, or never came up.
    Closed,
}

impl Phase {
    /// The `state` token: empty for a live Box, whose state herdr's own agent dot shows.
    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Live(_) => "",
            Self::Starting => "…",
            Self::WaitingForMachine => "◌",
            Self::Asleep => "◐",
            Self::Closed => "✕",
        }
    }

    /// The state herdr's agent dot shows for the Box's pane.
    #[must_use]
    pub const fn agent_state(self) -> &'static str {
        match self {
            Self::Live(Activity::Working) => "working",
            Self::Live(Activity::Waiting) => "blocked",
            Self::Live(Activity::Idle) => "idle",
            Self::Starting | Self::WaitingForMachine | Self::Asleep | Self::Closed => "unknown",
        }
    }
}

/// One open Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// The Box's name.
    pub name: String,
    /// `owner/repo`, when the Box works on a repo.
    pub repo: Option<String>,
    /// The branch the Box pushes.
    pub branch: Option<String>,
    /// Where the Box is in its life.
    pub phase: Phase,
}

impl Shown {
    /// The workspace label: `◈ repo/name`, the repo without its owner.
    #[must_use]
    pub fn label(&self) -> String {
        label(self.repo.as_deref(), &self.name)
    }
}

/// The workspace label for Box `name` on `repo`.
#[must_use]
pub fn label(repo: Option<&str>, name: &str) -> String {
    repo.map(|r| r.rsplit('/').next().unwrap_or(r)).map_or_else(
        || format!("{BOX_MARK} {name}"),
        |short| format!("{BOX_MARK} {short}/{name}"),
    )
}

/// The fields of a `show` Box view this front reads.
#[derive(Debug, Deserialize)]
struct Row {
    /// The Box's name.
    name: String,
    /// `owner/repo`.
    repo: Option<String>,
    /// The pushed branch.
    branch: Option<String>,
    /// The Box's lifecycle.
    lifecycle: BoxLifecycle,
    /// The agent's session state.
    session_state: SessionState,
    /// The agent, when the controller read it.
    agent: Option<AgentRow>,
    /// The user machine the Box claims.
    held_machine: Option<HeldRow>,
}

/// The fields of a `show` agent view this front reads.
#[derive(Debug, Deserialize)]
struct AgentRow {
    /// Whether the agent was launched.
    #[serde(default)]
    launched: bool,
    /// The question the agent waits on.
    question: Option<String>,
}

/// The fields of a `show` held machine this front reads.
#[derive(Debug, Deserialize)]
struct HeldRow {
    /// `held`, `asleep` or `waiting`.
    state: String,
}

/// The `value` of `show`: every open Box, or the one asked for.
#[derive(Debug, Deserialize)]
struct ShowValue {
    /// Every open Box, when no Box was named.
    #[serde(default)]
    boxes: Vec<Row>,
    /// The Box named.
    #[serde(rename = "box")]
    boxed: Option<Row>,
    /// The named Box's latest events, oldest first, when they were asked for.
    #[serde(default)]
    events: Option<Vec<Happened>>,
}

impl Row {
    /// Where the Box is in its life.
    fn phase(&self) -> Phase {
        match self.lifecycle {
            BoxLifecycle::Closed | BoxLifecycle::Rejected | BoxLifecycle::SetupFailed => {
                Phase::Closed
            }
            BoxLifecycle::Asleep => Phase::Asleep,
            BoxLifecycle::Checking | BoxLifecycle::Pending | BoxLifecycle::SettingUp => {
                Phase::Starting
            }
            _ if self
                .held_machine
                .as_ref()
                .is_some_and(|m| m.state == "waiting") =>
            {
                Phase::WaitingForMachine
            }
            // Setup ends seconds before the agent's terminal exists; an attach then finds none.
            BoxLifecycle::Ready if !self.agent.as_ref().is_some_and(|a| a.launched) => {
                Phase::Starting
            }
            _ if self.agent.as_ref().is_some_and(|a| a.question.is_some()) => {
                Phase::Live(Activity::Waiting)
            }
            BoxLifecycle::Ready => Phase::Live(match self.session_state {
                SessionState::Starting | SessionState::Working => Activity::Working,
                SessionState::Idle | SessionState::Ended => Activity::Idle,
            }),
        }
    }

    /// The row as the front sees it.
    fn shown(self) -> Shown {
        let phase = self.phase();
        Shown {
            name: self.name,
            repo: self.repo,
            branch: self.branch,
            phase,
        }
    }
}

/// Every open Box in a `show` value.
pub fn parse_list(value: Value) -> Result<Vec<Shown>, Error> {
    let shown: ShowValue = serde_json::from_value(value)?;
    Ok(shown.boxes.into_iter().map(Row::shown).collect())
}

/// The one Box in a `show <box>` value.
pub fn parse_one(value: Value) -> Result<Option<Shown>, Error> {
    let shown: ShowValue = serde_json::from_value(value)?;
    Ok(shown.boxed.map(Row::shown))
}

/// Every open Box the controller shows the caller.
pub fn open_boxes(client: &Client) -> Result<Vec<Shown>, Error> {
    let reply = client.request(request(json!({"cmd": "show"}))?, |_| {})?;
    if let Some(e) = reply.error {
        return Err(Error::Refused(e.message));
    }
    parse_list(reply.value.unwrap_or(Value::Null))
}

/// The open Box `name`; `None` once it is no longer open.
pub fn open_box(client: &Client, name: &str) -> Result<Option<Shown>, Error> {
    let reply = client.request(request(json!({"cmd": "show", "box": name}))?, |_| {})?;
    match (reply.error, reply.value) {
        (None, Some(value)) => parse_one(value),
        _ => Ok(None),
    }
}

/// Box `name` as `show` reads it, with its latest `events` events, oldest first; no Box once
/// the controller shows none by that name.
pub fn watch_box(
    client: &Client,
    name: &str,
    events: u32,
) -> Result<(Option<Shown>, Vec<Happened>), Error> {
    let wire = json!({"cmd": "show", "box": name, "events": events});
    let reply = client.request(request(wire)?, |_| {})?;
    match (reply.error, reply.value) {
        (None, Some(value)) => parse_watched(value),
        _ => Ok((None, Vec::new())),
    }
}

/// The Box and events in a `show <box>` value asked for its events.
pub fn parse_watched(value: Value) -> Result<(Option<Shown>, Vec<Happened>), Error> {
    let shown: ShowValue = serde_json::from_value(value)?;
    Ok((
        shown.boxed.map(Row::shown),
        shown.events.unwrap_or_default(),
    ))
}

/// Wakes the asleep Box `name`.
pub fn wake(client: &Client, name: &str) -> Result<(), Error> {
    let reply = client.request(request(wake_wire(name))?, |_| {})?;
    reply
        .error
        .map_or(Ok(()), |e| Err(Error::Refused(e.message)))
}

/// The wire form of the request that wakes Box `name`.
fn wake_wire(name: &str) -> Value {
    json!({"cmd": "wake", "box": name})
}

/// Closes Box `name` on the controller, the way `moor close` does without `--force`.
pub fn close(client: &Client, name: &str) -> Result<(), Error> {
    let reply = client.request(request(close_wire(name))?, |_| {})?;
    reply
        .error
        .map_or(Ok(()), |e| Err(Error::Refused(e.message)))
}

/// The wire form of the request that closes Box `name`.
fn close_wire(name: &str) -> Value {
    json!({"cmd": "close", "box": name})
}

/// A request from its wire form, so the front states only the fields it sets.
pub fn request(wire: Value) -> Result<Request, Error> {
    Ok(serde_json::from_value(wire)?)
}

#[cfg(test)]
mod tests;
