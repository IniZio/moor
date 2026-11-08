//! Agent activity: the one place that turns what herdr-core reports about the agent's terminal
//! into an agent state.

use std::time::Duration;

use crate::seam::clock::Ts;

mod first_run;
mod menu;
mod stall;
mod wait;

pub use menu::{menu_line, menu_options};
pub use stall::{stalled_for, STALL_AFTER};
pub use wait::{wait_overdue, WaitCheck};

/// How long a session counts as starting while the agent has reported no status.
pub const STARTUP_GRACE: Duration = Duration::from_secs(120);

/// What the agent is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    /// Launched; no report yet.
    Starting,
    /// Working on a prompt.
    Working,
    /// Waiting on an explicit prompt: a permission, a question menu or an elicitation dialog.
    NeedsInput {
        /// The prompt text, when one could be read.
        question: Option<String>,
        /// The menu choices on screen, in order; empty when there is no menu.
        options: Vec<String>,
    },
    /// Done and waiting for input.
    Idle,
    /// Ended its turn but waits on background work it started.
    Waiting {
        /// What it waits on, one line per piece.
        on: Vec<String>,
    },
    /// The agent process ended.
    Exited {
        /// Its exit code; none when it died by a signal or its terminal is gone.
        code: Option<i32>,
    },
    /// Not known right now.
    Unknown,
}

impl AgentState {
    /// Whether the agent should be treated as busy: starting, working or unknown.
    #[must_use]
    pub const fn busy(&self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Working | Self::Waiting { .. } | Self::Unknown
        )
    }

    /// The name shown to users.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Working => "working",
            Self::NeedsInput { .. } => "needs_input",
            Self::Idle => "idle",
            Self::Waiting { .. } => "waiting",
            Self::Exited { .. } => "exited",
            Self::Unknown => "unknown",
        }
    }
}

/// Where a reading came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The status the agent reported to its terminal.
    Status,
    /// The terminal's life: started, finished or gone.
    Terminal,
    /// A first-run screen the profile read on the terminal while the agent reports no status.
    FirstRun,
    /// The read timed out.
    Timeout,
}

impl Source {
    /// The name shown to users.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Terminal => "terminal",
            Self::FirstRun => "first_run",
            Self::Timeout => "timeout",
        }
    }
}

/// An agent state and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    /// The state.
    pub state: AgentState,
    /// Its source.
    pub source: Source,
}

/// Whether the agent's terminal still runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Life {
    /// The program runs.
    Live,
    /// The program ended.
    Finished {
        /// Its exit code; none when a signal ended it.
        exit: Option<i32>,
    },
}

/// Why a blocked agent waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// It asks to run a tool.
    Permission,
    /// It asks a question.
    Question,
}

/// The status the agent last reported to its terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Working; `doing` is the line it shows for the work.
    Working {
        /// What it does now, one short line.
        doing: Option<String>,
    },
    /// Its turn ended.
    Idle,
    /// Waiting on a person.
    Blocked {
        /// Why, when it said.
        kind: Option<BlockKind>,
        /// What it asks, one line.
        msg: Option<String>,
    },
    /// Its turn failed.
    Error {
        /// Why, one line.
        msg: Option<String>,
    },
}

/// What herdr-core reports about the agent's terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// Whether the program runs.
    pub life: Life,
    /// The last status; none until the agent reports one.
    pub status: Option<Status>,
    /// When the status last changed.
    pub status_at: Option<Ts>,
    /// How many turns have ended.
    pub turn: u64,
}

/// A question the agent asks, with its choices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The question.
    pub text: String,
    /// Its choices, in order.
    pub options: Vec<String>,
}

/// What the agent's hooks left beside the terminal for what the terminal cannot show.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Note {
    /// The agent's own session id.
    pub session_id: Option<String>,
    /// The agent's last reply, left when its turn ended.
    pub said: Option<String>,
    /// The question it asks now.
    pub question: Option<Question>,
    /// Background work its last turn left running, one line per piece.
    pub background: Vec<String>,
    /// Whether every piece of that work runs where the Box can see it end.
    pub background_local: bool,
}

/// One read of the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    clippy::large_enum_variant,
    reason = "a probe is held for one read and never stored"
)]
pub enum Probe {
    /// herdr-core answered.
    Read {
        /// The agent's terminal; none when there is none.
        term: Option<Term>,
        /// The hooks' note.
        note: Note,
        /// The terminal's screen, read while the agent is not working.
        screen: Option<String>,
    },
    /// The read failed or missed its deadline.
    TimedOut,
}

impl Probe {
    /// The terminal read, if any.
    #[must_use]
    pub const fn term(&self) -> Option<&Term> {
        match self {
            Self::Read { term, .. } => term.as_ref(),
            Self::TimedOut => None,
        }
    }

    /// The hooks' note, if read.
    #[must_use]
    pub const fn note(&self) -> Option<&Note> {
        match self {
            Self::Read { note, .. } => Some(note),
            Self::TimedOut => None,
        }
    }

    /// The screen, if read.
    #[must_use]
    pub fn screen(&self) -> Option<&str> {
        match self {
            Self::Read { screen, .. } => screen.as_deref(),
            Self::TimedOut => None,
        }
    }
}

/// Builds an activity.
const fn act(state: AgentState, source: Source) -> Activity {
    Activity { state, source }
}

/// Time elapsed from `then` to `now`, zero when `then` is later.
fn age(now: Ts, then: Ts) -> Duration {
    let ms = now.as_millis().saturating_sub(then.as_millis());
    Duration::from_millis(u64::try_from(ms).unwrap_or(0))
}

/// The state a blocked status gives, filled in from the note's question or the menu on screen.
fn blocked(kind: Option<BlockKind>, msg: Option<&str>, note: &Note, screen: &str) -> AgentState {
    let asked = note
        .question
        .as_ref()
        .filter(|_| kind != Some(BlockKind::Permission));
    AgentState::NeedsInput {
        question: asked.map(|q| q.text.clone()).or_else(|| {
            msg.map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_owned)
        }),
        options: asked.map_or_else(|| menu_options(screen), |q| q.options.clone()),
    }
}

/// Chooses the agent state from one read. `started` is when the agent session began;
/// `first_run` names the first-run screen the profile reads on the terminal, if any.
#[must_use]
pub fn decide(probe: &Probe, now: Ts, started: Ts, first_run: Option<&str>) -> Activity {
    let Probe::Read { term, note, screen } = probe else {
        return act(AgentState::Unknown, Source::Timeout);
    };
    let Some(term) = term else {
        return act(AgentState::Exited { code: None }, Source::Terminal);
    };
    if let Life::Finished { exit } = term.life {
        return act(AgentState::Exited { code: exit }, Source::Terminal);
    }
    let screen = screen.as_deref().unwrap_or("");
    let working = matches!(term.status, Some(Status::Working { .. }));
    if let Some(name) = first_run.filter(|_| !working) {
        return first_run::first_run_wait(name);
    }
    let state = match &term.status {
        Some(Status::Working { .. }) => AgentState::Working,
        Some(Status::Idle | Status::Error { .. }) if !note.background.is_empty() => {
            AgentState::Waiting {
                on: note.background.clone(),
            }
        }
        Some(Status::Idle | Status::Error { .. }) => AgentState::Idle,
        Some(Status::Blocked { kind, msg }) => blocked(*kind, msg.as_deref(), note, screen),
        None => {
            if age(now, started) < STARTUP_GRACE {
                return act(AgentState::Starting, Source::Terminal);
            }
            return act(AgentState::Unknown, Source::Terminal);
        }
    };
    act(state, Source::Status)
}

/// Whether a reading may move a Working Box to Idle: only one the agent's own status gives, an
/// ended agent, a first-run screen, or no live session (`probe` is `None`).
#[must_use]
pub const fn may_apply_quiet(activity: &Activity, probe: Option<&Probe>) -> bool {
    if probe.is_none() || matches!(activity.state, AgentState::Exited { .. }) {
        return true;
    }
    matches!(activity.source, Source::Status | Source::FirstRun)
        && matches!(
            activity.state,
            AgentState::Idle | AgentState::NeedsInput { .. }
        )
}

#[cfg(test)]
mod tests;
