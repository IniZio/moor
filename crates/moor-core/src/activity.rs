//! Agent activity: the one place that turns raw probe readings into an agent state.

use std::time::Duration;

use crate::seam::clock::Ts;

mod first_run;
mod menu;

pub use menu::menu_options;

/// How long a cached hook reading stands in for a dropped read.
pub const HOOK_TTL: Duration = Duration::from_secs(120);
/// How long a session counts as starting while no hook has reported.
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
    /// The agent process ended.
    Exited {
        /// Its exit code.
        code: i32,
    },
    /// Not known right now.
    Unknown,
}

impl AgentState {
    /// Whether the agent should be treated as busy: starting, working or unknown.
    #[must_use]
    pub const fn busy(&self) -> bool {
        matches!(self, Self::Starting | Self::Working | Self::Unknown)
    }

    /// The name shown to users.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Working => "working",
            Self::NeedsInput { .. } => "needs_input",
            Self::Idle => "idle",
            Self::Exited { .. } => "exited",
            Self::Unknown => "unknown",
        }
    }
}

/// Where a reading came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A fresh hook report.
    Hooks,
    /// A cached hook report standing in for a dropped read.
    HooksCached,
    /// The terminal multiplexer's status.
    Herdr,
    /// The pane text.
    Pane,
    /// A read timed out and no cache stood in.
    Timeout,
    /// A first-run screen the profile read on the pane while no hook has reported since launch.
    FirstRun,
}

impl Source {
    /// The name shown to users.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hooks => "hooks",
            Self::HooksCached => "hooks_cached",
            Self::Herdr => "herdr",
            Self::Pane => "pane",
            Self::Timeout => "timeout",
            Self::FirstRun => "first_run",
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

/// What a hook report says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookKind {
    /// The session started; no prompt yet.
    Starting {
        /// The launch that wrote this marker; `None` when the marker names none.
        launch: Option<String>,
    },
    /// A prompt is being worked on.
    Busy,
    /// The agent finished and waits for input.
    Idle,
    /// The agent waits on an answer.
    Blocked,
}

/// One hook report, neutral across agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRecord {
    /// What it reports.
    pub kind: HookKind,
    /// The agent session it came from.
    pub session_id: Option<String>,
    /// The text the agent attached: the question it waits on, or its last message when idle.
    pub message: Option<String>,
    /// The choices the agent offered with its question, in order; empty when it named none.
    pub options: Vec<String>,
    /// When the agent wrote it.
    pub at: Ts,
}

/// The last hook report seen for a Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCache {
    /// The report.
    pub record: HookRecord,
    /// When it was read.
    pub at: Ts,
    /// Whether a `needs_input` event already reported this record.
    pub reported: bool,
}

/// The multiplexer's view of a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneStatus {
    /// Working.
    Working,
    /// Idle.
    Idle,
    /// Blocked on input.
    Blocked,
    /// No status.
    Unknown,
}

/// One pane observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneObs {
    /// The multiplexer's status.
    pub status: PaneStatus,
    /// The end of the pane text.
    pub tail: String,
    /// The line the agent shows as its pending question, as its profile reads it from the text.
    pub prompt: Option<String>,
    /// The exit code from the exit marker line, when the agent ended.
    pub exit_marker: Option<i32>,
}

/// Result of reading the hook file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookRead {
    /// A report was read.
    Fresh(HookRecord),
    /// No report is there.
    Absent,
    /// The read missed its deadline or failed.
    TimedOut,
}

/// Result of observing the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneRead {
    /// The pane was observed.
    Fresh(PaneObs),
    /// The pane no longer exists.
    Gone,
    /// There is no pane to observe.
    Absent,
    /// The observation missed its deadline or failed.
    TimedOut,
}

/// Everything one probe read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probes {
    /// The hook file.
    pub hook: HookRead,
    /// The pane.
    pub pane: PaneRead,
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

/// How many trailing non-empty pane lines can show a background marker.
const MARKER_LINES: usize = 6;

/// Whether the last lines of the pane text show background work in progress.
fn background_running(tail: &str) -> bool {
    let lines: Vec<&str> = tail.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(MARKER_LINES);
    lines.iter().skip(start).any(|line| {
        let lower = line.to_lowercase();
        let words: Vec<&str> = lower.split_whitespace().collect();
        words.windows(3).any(|w| {
            matches!(w, [n, "background", "task" | "tasks" | "agent" | "agents"]
                if n.parse::<u32>().is_ok_and(|n| n > 0))
        })
    })
}

/// The pane text, empty when no pane was observed.
fn tail_of(pane: &PaneRead) -> &str {
    match pane {
        PaneRead::Fresh(obs) => &obs.tail,
        _ => "",
    }
}

/// The pending question the profile read from the pane, if any.
fn prompt_of(pane: &PaneRead) -> Option<&str> {
    match pane {
        PaneRead::Fresh(obs) => obs.prompt.as_deref(),
        _ => None,
    }
}

/// An ended or vanished pane beats every other reading.
const fn pane_ended(pane: &PaneRead) -> Option<Activity> {
    match pane {
        PaneRead::Gone => Some(act(AgentState::Idle, Source::Pane)),
        PaneRead::Fresh(PaneObs {
            exit_marker: Some(code),
            ..
        }) => Some(act(AgentState::Exited { code: *code }, Source::Pane)),
        _ => None,
    }
}

/// The state a hook report gives, if it still counts; see [`decide`] for `first_run`.
fn from_record(
    rec: &HookRecord,
    source: Source,
    young: bool,
    pane: &PaneRead,
    first_run: Option<&str>,
) -> Option<Activity> {
    let tail = tail_of(pane);
    match &rec.kind {
        HookKind::Busy => Some(act(AgentState::Working, source)),
        HookKind::Blocked => Some(act(
            AgentState::NeedsInput {
                question: [rec.message.as_deref(), prompt_of(pane)]
                    .into_iter()
                    .flatten()
                    .map(str::trim)
                    .find(|m| !m.is_empty())
                    .map(str::to_owned),
                options: if rec.options.is_empty() {
                    menu_options(tail)
                } else {
                    rec.options.clone()
                },
            },
            source,
        )),
        HookKind::Idle if background_running(tail) => Some(act(AgentState::Working, Source::Pane)),
        HookKind::Idle => Some(act(AgentState::Idle, source)),
        HookKind::Starting { .. } => first_run
            .map(first_run::first_run_wait)
            .or_else(|| young.then(|| act(AgentState::Working, source))),
    }
}

/// The state a cached report gives, while it is younger than the TTL.
fn from_cache(
    cache: Option<&HookCache>,
    now: Ts,
    young: bool,
    pane: &PaneRead,
    first_run: Option<&str>,
) -> Option<Activity> {
    let c = cache.filter(|c| age(now, c.at) < HOOK_TTL)?;
    from_record(&c.record, Source::HooksCached, young, pane, first_run)
}

/// The state the pane alone gives.
fn from_pane(pane: &PaneRead) -> Activity {
    match pane {
        PaneRead::Fresh(obs) => from_obs(obs),
        PaneRead::TimedOut => act(AgentState::Unknown, Source::Timeout),
        PaneRead::Gone | PaneRead::Absent => act(AgentState::Unknown, Source::Pane),
    }
}

/// The state one pane observation gives.
fn from_obs(obs: &PaneObs) -> Activity {
    let quiet = matches!(obs.status, PaneStatus::Idle | PaneStatus::Unknown);
    if quiet && background_running(&obs.tail) {
        return act(AgentState::Working, Source::Pane);
    }
    match obs.status {
        PaneStatus::Working => act(AgentState::Working, Source::Herdr),
        PaneStatus::Idle => act(AgentState::Idle, Source::Herdr),
        PaneStatus::Blocked => act(
            AgentState::NeedsInput {
                question: obs.prompt.clone().filter(|p| !p.trim().is_empty()),
                options: menu_options(&obs.tail),
            },
            Source::Herdr,
        ),
        PaneStatus::Unknown => act(AgentState::Unknown, Source::Pane),
    }
}

/// Chooses the agent state from one probe reading.
///
/// A dropped hook read never falls through to the pane; only a hook that is absent
/// does. `started` is when the agent session began. `first_run` names the profile's first-run screen.
#[must_use]
pub fn decide(
    probes: &Probes,
    cache: Option<&HookCache>,
    now: Ts,
    started: Ts,
    first_run: Option<&str>,
) -> Activity {
    if let Some(a) = pane_ended(&probes.pane) {
        return a;
    }
    let young = age(now, started) < STARTUP_GRACE;
    let pane = &probes.pane;
    match &probes.hook {
        HookRead::Fresh(rec) => {
            if let Some(a) = from_record(rec, Source::Hooks, young, pane, first_run) {
                return a;
            }
        }
        HookRead::TimedOut => {
            return from_cache(cache, now, young, pane, first_run)
                .unwrap_or_else(|| act(AgentState::Unknown, Source::Timeout));
        }
        HookRead::Absent => {
            if let Some(a) = from_cache(cache, now, young, pane, first_run) {
                return a;
            }
        }
    }
    if young {
        return act(AgentState::Starting, Source::Hooks);
    }
    from_pane(&probes.pane)
}

/// Whether a reading may move a Working Box to Idle.
///
/// Only a hook report (fresh or cached) of idle or blocked, a first-run screen, an exited or
/// vanished pane, or no live session (`probes` is `None`) may; pane evidence alone never does.
#[must_use]
pub const fn may_apply_quiet(activity: &Activity, probes: Option<&Probes>) -> bool {
    let Some(probes) = probes else {
        return true;
    };
    if matches!(probes.pane, PaneRead::Gone) || matches!(activity.state, AgentState::Exited { .. })
    {
        return true;
    }
    matches!(
        activity.source,
        Source::Hooks | Source::HooksCached | Source::FirstRun
    ) && matches!(
        activity.state,
        AgentState::Idle | AgentState::NeedsInput { .. }
    )
}

#[cfg(test)]
mod tests;
