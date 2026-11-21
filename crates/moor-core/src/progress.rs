//! A Box as a person sees it: one state derived from a Box's row and events, the line that
//! matters now, a PR link and the timeline. Every front shows what [`fn@derive`] says; none
//! computes its own.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::view::progress::{latest, span, unix, Happened};
use crate::view::{ApprovalView, BoxView, EventView};
use failed::closed_failure;

mod failed;
mod gate;
mod markdown;
mod needs;
mod timeline;
pub use gate::gate_ask_id;
pub use needs::{approve_needs, ask_id, needs_of, Approve, NeedKind, Needs, Pick, Role};
pub use timeline::{timeline, timeline_with, Class, TimelineEntry};
mod title;
pub mod transcript;
pub use title::{title_of, TITLE_MAX};

/// How long a working Box may go without an event before it shows a warning.
pub const STALE_AFTER_MS: i64 = 30 * 60 * 1000;

/// Where a Box is, in the words a person uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Progress {
    /// Waits for a free machine slot.
    Queued,
    /// Its machine is being acquired and set up.
    SettingUp,
    /// The agent works.
    Working,
    /// The agent asked a question or an account problem blocks it; only a person can go on.
    NeedsYou,
    /// The agent finished its turn and is quiet.
    Done,
    /// The Box sleeps.
    Asleep,
    /// The Box is closed.
    Closed,
    /// Setup failed, the agent could not launch or crashed, or the Box was refused.
    Failed,
}

/// A GitHub pull request the agent named, on one of the Box's own repos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrLink {
    /// `OWNER/NAME`, as the Box's repo spells it.
    pub repo: String,
    /// The pull request number.
    pub number: u64,
}

/// What a front shows of one Box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressView {
    /// A short name for the Box: the start of what it was asked, else the Box name.
    #[serde(default)]
    pub title: String,
    /// The derived state.
    pub state: Progress,
    /// Setup failed, the agent died or its session ended badly, or the run failed; a closed Box keeps it.
    #[serde(default)]
    pub failed: bool,
    /// A working Box with no event for [`STALE_AFTER_MS`].
    pub stale: bool,
    /// The one line that matters now.
    pub line: String,
    /// When the newest event came, RFC 3339 UTC; none without events.
    pub last_event_at: Option<String>,
    /// The PR the agent named, newest first.
    pub pr: Option<PrLink>,
    /// What the agent asks, when it waits at a menu a person can answer.
    pub needs: Option<Needs>,
}

/// The Box `boxed` is at `now_ms` (unix milliseconds), given its events oldest first.
#[must_use]
pub fn derive(boxed: &BoxView, events: &[EventView], now_ms: i64) -> ProgressView {
    let state = state_of(boxed);
    let newest = events.last().and_then(|e| unix(&e.ts));
    let silent = newest.map(|s| now_ms - s * 1000);
    let happened: Vec<Happened> = events
        .iter()
        .map(|e| Happened {
            kind: e.kind.clone(),
            payload: e.payload.clone(),
            ts: e.ts.clone(),
        })
        .collect();
    ProgressView {
        title: title_of(boxed.prompt.as_deref(), &boxed.name),
        state,
        failed: state == Progress::Failed
            || state == Progress::Closed && closed_failure(events).is_some(),
        stale: state == Progress::Working && silent.is_some_and(|s| s > STALE_AFTER_MS),
        line: line_of(state, boxed, events, &happened, silent),
        last_event_at: events.last().map(|e| e.ts.clone()),
        pr: pr_of(boxed, events),
        needs: (state == Progress::NeedsYou)
            .then(|| {
                approval_of(boxed).map_or_else(
                    || gate::needs(boxed).or_else(|| boxed.agent.as_ref().and_then(needs_of)),
                    |a| Some(approve_needs(a)),
                )
            })
            .flatten(),
    }
}

/// The state the Box's row says. Events never decide it: a window of them can miss the one that
/// would.
fn state_of(b: &BoxView) -> Progress {
    if approval_of(b).is_some() {
        return Progress::NeedsYou;
    }
    if let Some(forced) = gate::forced(b) {
        return forced;
    }
    match b.lifecycle {
        BoxLifecycle::Closed => Progress::Closed,
        BoxLifecycle::Rejected | BoxLifecycle::SetupFailed => Progress::Failed,
        BoxLifecycle::Pending => Progress::Queued,
        BoxLifecycle::Checking | BoxLifecycle::SettingUp => Progress::SettingUp,
        BoxLifecycle::Asleep => Progress::Asleep,
        BoxLifecycle::Ready => ready_state(b),
    }
}

/// The approve step a run waits at in `b`, while the Box is open: the agent has ended its turn
/// for it, so the Box may be asleep, and only the run's owner can go on.
fn approval_of(b: &BoxView) -> Option<&ApprovalView> {
    b.approval
        .as_ref()
        .filter(|_| matches!(b.lifecycle, BoxLifecycle::Ready | BoxLifecycle::Asleep))
}

/// The state of a Box whose machine is up.
fn ready_state(b: &BoxView) -> Progress {
    let agent = b.agent.as_ref();
    match agent.map(|a| a.state.as_str()) {
        Some("needs_input") => return Progress::NeedsYou,
        Some("failed") => return Progress::Failed,
        Some("exited") if agent.and_then(|a| a.exit_code).is_some_and(|c| c != 0) => {
            return Progress::Failed;
        }
        // It ended its turn but waits on work it started: still going.
        Some("waiting") => return Progress::Working,
        _ => {}
    }
    if !b.attention.is_empty() {
        return Progress::NeedsYou;
    }
    match b.session_state {
        SessionState::Starting | SessionState::Working => Progress::Working,
        SessionState::Idle | SessionState::Ended if gate::holds(b) => Progress::Working,
        SessionState::Idle | SessionState::Ended => Progress::Done,
    }
}

/// The text of event `kind`'s payload field `key`, when it has one.
fn text<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

/// The first line of `s` that says something, as plain text cut to 140 characters. The agent
/// writes markdown; a one-line summary shows none of it.
fn first_line(s: &str) -> String {
    let line = markdown::first_plain_line(s);
    let mut out: String = line.chars().take(140).collect();
    if line.chars().count() > 140 {
        out.push('…');
    }
    out
}

/// The newest event of `kind`, with its text under `key`.
fn newest<'a>(events: &'a [EventView], kind: &str, key: &str) -> Option<&'a str> {
    events
        .iter()
        .rev()
        .filter(|e| e.kind == kind)
        .find_map(|e| text(&e.payload, key))
}

/// The line for a Box in `state`.
fn line_of(
    state: Progress,
    b: &BoxView,
    events: &[EventView],
    happened: &[Happened],
    silent: Option<i64>,
) -> String {
    if let Some(line) = gate::line(state, b) {
        return line;
    }
    let said = || newest(events, "agent_said", "text").map(first_line);
    match state {
        Progress::Queued => "waiting for a machine slot".to_owned(),
        Progress::SettingUp => latest(happened)
            .map_or_else(|| "setting up".to_owned(), |s| format!("setting up · {s}")),
        Progress::NeedsYou => approval_of(b)
            .map(|a| a.question.clone())
            .or_else(|| b.agent.as_ref().and_then(|a| a.question.clone()))
            .or_else(|| newest(events, "needs_input", "question").map(str::to_owned))
            .or_else(|| b.attention.first().map(|a| a.message.clone()))
            .map_or_else(|| "needs your input".to_owned(), |q| first_line(&q)),
        Progress::Working => {
            let doing = newest(events, "agent_doing", "text").map(first_line);
            let now = doing.or_else(said).unwrap_or_else(|| "working".to_owned());
            match silent.filter(|s| *s > STALE_AFTER_MS) {
                Some(s) => format!("no event for {} · last: {now}", span(s / 1000)),
                None => now,
            }
        }
        Progress::Done => match pr_of(b, events) {
            Some(pr) => format!("PR #{}", pr.number),
            None => said().unwrap_or_else(|| "done".to_owned()),
        },
        Progress::Asleep => "asleep".to_owned(),
        Progress::Closed => closed_failure(events)
            .and_then(|e| timeline(std::slice::from_ref(e)).pop())
            .map_or_else(|| "closed".to_owned(), |e| first_line(&e.text)),
        Progress::Failed => failure(b, happened)
            .or_else(|| b.state_reason.clone())
            .map_or_else(|| "failed".to_owned(), |why| first_line(&why)),
    }
}

/// Why a failed Box failed: the agent's launch failure, else the latest failure event.
fn failure(b: &BoxView, events: &[Happened]) -> Option<String> {
    if let Some(reason) = b.agent.as_ref().and_then(|a| a.reason.as_ref()) {
        return Some(reason.text());
    }
    let why = crate::view::progress::failure(events)?;
    Some(if why.contains("failed") {
        why
    } else {
        format!("failed: {why}")
    })
}

/// The PR the newest agent message that names one of the Box's repos' pulls points at.
fn pr_of(b: &BoxView, events: &[EventView]) -> Option<PrLink> {
    let repos: Vec<String> = b
        .repo
        .iter()
        .chain(&b.other_repos)
        .map(|r| format!("{}/{}", r.owner, r.name))
        .collect();
    events
        .iter()
        .rev()
        .filter(|e| e.kind == "agent_said")
        .filter_map(|e| text(&e.payload, "text"))
        .find_map(|said| pr_in(said, &repos))
}

/// The last `github.com/OWNER/NAME/pull/N` in `said` whose repo is one of `repos`. The link is
/// rebuilt from the parts; no text of the agent's goes into one.
fn pr_in(said: &str, repos: &[String]) -> Option<PrLink> {
    const HOST: &str = "github.com/";
    let mut found = None;
    for (at, _) in said.match_indices(HOST) {
        let Some(rest) = said.get(at + HOST.len()..) else {
            continue;
        };
        let mut parts = rest.split('/');
        let (Some(owner), Some(name), Some("pull"), Some(num)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let digits: String = num.chars().take_while(char::is_ascii_digit).collect();
        let (Ok(number), Some(repo)) = (
            digits.parse::<u64>(),
            repos
                .iter()
                .find(|r| r.eq_ignore_ascii_case(&format!("{owner}/{name}"))),
        ) else {
            continue;
        };
        found = Some(PrLink {
            repo: repo.clone(),
            number,
        });
    }
    found
}

#[cfg(test)]
mod tests;
