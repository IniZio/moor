//! Keeps one status message per Slack thread current as its Task's events arrive.

use std::collections::HashMap;
use std::fmt::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moor_core::seam::clock::Clock;
use moor_core::secret::Secret;
use serde_json::Value;

use super::api::Api;
use super::pr::{PrCache, PrOf};
use super::prompt::Pending;
use super::proof::Proofs;
use crate::adapters::clock::SystemClock;
use crate::env::Env;
use crate::store::{SlackThread, Store, STORE_FILE};

/// Least time between two sends for one thread; changes in between coalesce.
const UPDATE_EVERY: Duration = Duration::from_secs(2);
/// Time between polls of the event feed.
const POLL_EVERY: Duration = Duration::from_millis(500);
/// Events read per feed query.
const BATCH: usize = 500;
/// Longest question shown, in characters.
const QUESTION_MAX: usize = 300;

/// Where the Task is in its life.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) enum State {
    /// Created or setting up; not yet working.
    #[default]
    Starting,
    /// The agent is working.
    Working,
    /// The agent is quiet.
    Idle,
    /// The Task is asleep.
    Asleep,
    /// The agent asked a question.
    NeedsInput(String),
    /// The Task closed.
    Closed,
    /// Admission refused the Task.
    Rejected(String),
    /// Box setup failed.
    SetupFailed(String),
}

/// What is known about the Task's Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BoxState {
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

/// The status of one Task, folded from its events.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct View {
    /// Task state.
    pub state: State,
    /// Box state, once any box event arrived.
    pub boxed: Option<BoxState>,
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
            "box_requested" => self.boxed = Some(BoxState::Requested),
            "box_ready" => self.boxed = Some(BoxState::Ready),
            "box_failed" => self.boxed = Some(BoxState::Failed),
            "box_asleep" => self.boxed = Some(BoxState::Asleep),
            "box_gone" => self.boxed = Some(BoxState::Gone),
            "box_deleted" => self.boxed = Some(BoxState::Deleted),
            _ => {}
        }
    }
}

/// Escapes the characters Slack treats as markup, so a quoted line cannot ping anyone.
pub(super) fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The message text for `task`; names the loopback web UI when `web` is known
/// and links the Task branch's open pull request when `pr` is known.
pub(super) fn render(task: &str, view: &View, web: Option<SocketAddr>, pr: Option<&str>) -> String {
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
    let mut text = format!("*{}*: {state}", escape(task));
    if let Some(b) = view.boxed {
        let b = match b {
            BoxState::Requested => "requested",
            BoxState::Ready => "ready",
            BoxState::Failed => "failed",
            BoxState::Asleep => "asleep",
            BoxState::Gone => "gone",
            BoxState::Deleted => "deleted",
        };
        let _ = write!(text, "\nBox: {b}");
    }
    if let Some(url) = pr {
        let _ = write!(text, "\n<{}|Pull request>", escape(url).replace('|', "%7C"));
    }
    if let Some(addr) = web {
        let _ = write!(
            text,
            "\nWeb UI: http://{addr}/ (loopback on the moor host; reach it with `ssh -L {port}:{addr} <host>`)",
            port = addr.port()
        );
    }
    text
}

/// A thread's identity: channel and thread timestamp.
pub(super) type Key = (String, String);

/// One send the planner wants made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Step {
    /// Thread the message belongs to.
    pub key: Key,
    /// The message to edit; none means post a new one.
    pub ts: Option<String>,
    /// Text to send.
    pub text: String,
}

/// What was last sent for a thread.
#[derive(Default)]
struct Sent {
    /// Text of the last successful send.
    text: Option<String>,
    /// When the last send was tried.
    at: Option<Instant>,
}

/// Decides which threads need a send now, honoring the per-thread rate limit.
#[derive(Default)]
pub(super) struct Planner {
    /// Send history by thread.
    sent: HashMap<Key, Sent>,
}

impl Planner {
    /// Sends due at `now`: a post for a thread with no message yet, an edit for changed text.
    pub(super) fn plan(
        &self,
        threads: &[SlackThread],
        views: &HashMap<String, View>,
        web: Option<SocketAddr>,
        prs: &HashMap<String, String>,
        now: Instant,
    ) -> Vec<Step> {
        let mut steps = Vec::new();
        for t in threads {
            let key = (t.channel.clone(), t.thread_ts.clone());
            let view = views.get(t.task.as_str()).cloned().unwrap_or_default();
            let text = render(
                t.task.as_str(),
                &view,
                web,
                prs.get(t.task.as_str()).map(String::as_str),
            );
            let sent = self.sent.get(&key);
            if t.status_ts.is_some() && sent.and_then(|s| s.text.as_ref()) == Some(&text) {
                continue;
            }
            if sent
                .and_then(|s| s.at)
                .is_some_and(|at| now.duration_since(at) < UPDATE_EVERY)
            {
                continue;
            }
            steps.push(Step {
                key,
                ts: t.status_ts.clone(),
                text,
            });
        }
        steps
    }

    /// Notes a send tried at `now`; `ok` keeps the text as sent, a failure leaves it due.
    pub(super) fn record(&mut self, step: &Step, now: Instant, ok: bool) {
        let entry = self.sent.entry(step.key.clone()).or_default();
        entry.at = Some(now);
        if ok {
            entry.text = Some(step.text.clone());
        }
    }
}

/// Follows the event feed for the life of the process, keeping thread status messages current.
pub(crate) fn follow(
    env: &Env,
    api: &Api,
    bot: &Secret,
    web: Option<SocketAddr>,
    pr_of: &PrOf,
    log: &(dyn Fn(&str) + Send + Sync),
) {
    let clock: Arc<dyn Clock> = loop {
        match SystemClock::from_env(env) {
            Ok(clock) => break Arc::new(clock),
            Err(e) => {
                log(&format!("slack status: {e}"));
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    };
    let store = loop {
        match open_store(env, &clock) {
            Ok(store) => break store,
            Err(e) => {
                log(&format!("slack status: {e}"));
                clock.sleep(Duration::from_secs(5));
            }
        }
    };
    let mut views: HashMap<String, View> = HashMap::new();
    let mut pending = Pending::default();
    let mut planner = Planner::default();
    let mut proofs = Proofs::default();
    let mut prs = PrCache::default();
    let mut cursor = 0;
    loop {
        match store.events_after(cursor, BATCH) {
            Ok(events) => {
                for e in &events {
                    cursor = cursor.max(e.row.id);
                    views
                        .entry(e.task_name.clone())
                        .or_default()
                        .fold(&e.row.kind, &e.row.payload);
                    proofs.fold(&e.task_name, &e.row.kind, &e.row.payload);
                    pending.fold(e.row.id, &e.task_name, &e.row.kind, &e.row.payload);
                }
                if events.len() == BATCH {
                    continue;
                }
            }
            Err(e) => log(&format!("slack status: {e}")),
        }
        match store.slack_threads() {
            Ok(threads) => {
                prs.refresh(&threads, &views, pr_of, clock.instant());
                for step in planner.plan(&threads, &views, web, prs.urls(), clock.instant()) {
                    let ok = send(&store, api, bot, &step, log);
                    planner.record(&step, clock.instant(), ok);
                }
                pending.post_due(&store, api, bot, &threads, log);
                match env.state_dir() {
                    Some(dir) => proofs.post_due(&store, api, bot, &threads, &dir, log),
                    None => log("slack status: no state directory, proofs not posted"),
                }
            }
            Err(e) => log(&format!("slack status: {e}")),
        }
        clock.sleep(POLL_EVERY);
    }
}

/// Performs one send and returns whether it succeeded; failures are logged.
fn send(store: &Store, api: &Api, bot: &Secret, step: &Step, log: &dyn Fn(&str)) -> bool {
    let (channel, thread_ts) = &step.key;
    let result = step.ts.as_ref().map_or_else(
        || {
            api.post_message(bot, channel, thread_ts, &step.text)
                .map_err(|e| e.to_string())
                .and_then(|ts| {
                    store
                        .set_slack_status_ts(channel, thread_ts, &ts)
                        .map_err(|e| e.to_string())
                })
        },
        |ts| {
            api.update_message(bot, channel, ts, &step.text)
                .map_err(|e| e.to_string())
        },
    );
    if let Err(e) = &result {
        log(&format!("slack status: {e}"));
    }
    result.is_ok()
}

/// Opens the store this process shares with the controller.
fn open_store(env: &Env, clock: &Arc<dyn Clock>) -> Result<Store, String> {
    let dir = env
        .state_dir()
        .ok_or("no state directory: set state_dir or MOOR_STATE_DIR")?;
    Store::open(&dir.join(STORE_FILE), Arc::clone(clock)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
