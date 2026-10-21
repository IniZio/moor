//! The coordinator's turns: the scheduler takes the starter's waiting messages as one turn per
//! thread, at most [`MAX_TURNS`] at once, and each turn posts a placeholder, runs the agent on
//! the starter's credential, stores its line and then posts it.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_client::Client;
use moor_core::secret::Secret;

use crate::api::{Api, ApiError};
use crate::coordinator::brief::BRIEF;
use crate::coordinator::context::{self, Parts, Said};
use crate::coordinator::turn::{self, Ended, Turn, LIMITS};
use crate::coordinator::{Agent, Answer, TurnSetup};
use crate::credentials::{self, daemon_uid, private_dir, SlackUser};
use crate::db::{BoxState, Db, End, TurnRow};
use crate::Log;

mod host;
mod lines;

/// Most turns that run at once.
const MAX_TURNS: usize = 4;

/// Longest the scheduler sleeps before it looks for waiting messages again.
const TICK: Duration = Duration::from_millis(500);

/// What the placeholder says while a turn runs.
const PLACEHOLDER: &str = "…";

/// Waits before each new try of a line Slack did not take for a reason it may get over.
const BACKOFF: [Duration; 3] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
];

/// Longest wait between tries, whatever `Retry-After` asks.
const MAX_WAIT: Duration = Duration::from_secs(30);

/// Slack's answers that the placeholder cannot be edited; the line is posted new instead.
const EDIT_REFUSED: [&str; 2] = ["message_not_found", "cant_update_message"];

/// Everything a turn needs, shared by the scheduler and the turns it runs.
pub(crate) struct Turns {
    /// Path of `slack.db`; the scheduler and each turn open their own connection.
    pub(crate) db_path: PathBuf,
    /// The front's state dir, which holds credentials, config dirs and workdirs.
    pub(crate) state_dir: PathBuf,
    /// The checked coordinator agent, or why it cannot run.
    pub(crate) agent: Result<Arc<dyn Agent>, String>,
    /// The model each turn runs on.
    pub(crate) model: String,
    /// Most bytes of context a turn gets.
    pub(crate) budget: usize,
    /// Web API client.
    pub(crate) api: Arc<Api>,
    /// Bot token for posting.
    pub(crate) bot_token: Arc<Secret>,
    /// Controller socket client.
    pub(crate) client: Client,
    /// Daemon log.
    pub(crate) log: Log,
    /// The current time in unix seconds.
    pub(crate) clock: fn() -> i64,
}

impl Turns {
    /// Runs the scheduler: takes waiting turns while fewer than [`MAX_TURNS`] run, each on its
    /// own thread, and looks again when one ends or every [`TICK`].
    pub(crate) fn run(self: Arc<Self>) {
        let db = match Db::open(&self.db_path) {
            Ok(db) => db,
            Err(e) => return (self.log)(&format!("turns: open slack.db: {e}")),
        };
        let (done_tx, done_rx) = mpsc::channel();
        let mut running = 0_usize;
        loop {
            while running < MAX_TURNS {
                match db.take_turn((self.clock)()) {
                    Ok(Some(row)) => {
                        running += 1;
                        let (me, done) = (Arc::clone(&self), done_tx.clone());
                        thread::spawn(move || {
                            me.take(&row);
                            let _ = done.send(());
                        });
                    }
                    Ok(None) => break,
                    Err(e) => {
                        (self.log)(&format!("turns: slack.db: {e}"));
                        break;
                    }
                }
            }
            if done_rx.recv_timeout(TICK).is_ok() {
                running -= 1;
                running -= done_rx.try_iter().count();
            }
        }
    }

    /// Runs turn `row` to its posted line.
    fn take(&self, row: &TurnRow) {
        let db = match Db::open(&self.db_path) {
            Ok(db) => db,
            Err(e) => return (self.log)(&format!("turn {}: open slack.db: {e}", row.id)),
        };
        let posted =
            self.api
                .post_message(&self.bot_token, &row.channel, &row.thread_ts, PLACEHOLDER);
        let placeholder = posted
            .inspect_err(|e| (self.log)(&format!("slack: placeholder failed: {e}")))
            .ok();
        if let Some(ts) = &placeholder {
            self.written(db.set_placeholder(row.id, ts));
        }
        let closing = self.outcome(&db, row);
        let stored = db.store_reply(
            row.id,
            closing.end,
            &closing.line,
            closing.bytes,
            closing.cost,
        );
        self.written(stored);
        if self.post(row, placeholder.as_deref(), &closing.line) {
            self.written(db.mark_posted(row.id, (self.clock)()));
        }
    }

    /// Edits the placeholder into `line`, or posts `line` when there is no placeholder or Slack
    /// says it cannot be edited. A failure Slack may get over is tried again after a wait.
    /// True once the line landed.
    fn post(&self, row: &TurnRow, placeholder: Option<&str>, line: &str) -> bool {
        let token = &self.bot_token;
        let (mut target, mut waits) = (placeholder, BACKOFF.into_iter());
        loop {
            let sent = target.map_or_else(
                || {
                    let posted = self
                        .api
                        .post_message(token, &row.channel, &row.thread_ts, line);
                    posted.map(drop)
                },
                |ts| self.api.update_message(token, &row.channel, ts, line),
            );
            let Err(error) = sent else {
                return true;
            };
            (self.log)(&format!("slack: turn reply failed: {error}"));
            let wait = match &error {
                ApiError::Slack(code) if target.is_some() && EDIT_REFUSED.contains(&&**code) => {
                    target = None;
                    continue;
                }
                ApiError::Slack(_) => None,
                ApiError::RateLimited(asked) => waits.next().map(|w| asked.unwrap_or(w)),
                ApiError::Transport(_) => waits.next(),
            };
            let Some(wait) = wait else {
                return false;
            };
            thread::sleep(wait.min(MAX_WAIT));
        }
    }

    /// Logs a failed `slack.db` write.
    fn written(&self, write: rusqlite::Result<()>) {
        if let Err(e) = write {
            (self.log)(&format!("turns: slack.db: {e}"));
        }
    }

    /// Runs the turn when it can run, and says how it ended.
    fn outcome(&self, db: &Db, row: &TurnRow) -> Closing {
        let agent = match &self.agent {
            Ok(agent) => Arc::clone(agent),
            Err(why) => return Closing::of(End::Failed, lines::unavailable(why)),
        };
        let Some(user) = SlackUser::parse(&row.owner) else {
            return Closing::of(
                End::Failed,
                lines::unavailable("the starter has no Slack user id"),
            );
        };
        let credential = match credentials::load(&self.state_dir, agent.kind(), &user) {
            Ok(Some(credential)) => credential,
            Ok(None) => return Closing::of(End::NoToken, lines::no_token(&row.owner)),
            Err(e) => return Closing::of(End::Failed, lines::unavailable(&e)),
        };
        let ready = credentials::config_dir(&self.state_dir, agent.kind(), &user)
            .and_then(|config| Ok((config, workdir(&self.state_dir, row)?)));
        let (config_dir, workdir) = match ready {
            Ok(dirs) => dirs,
            Err(e) => return Closing::of(End::Failed, lines::unavailable(&e)),
        };
        if let Some(why) = agent.unsafe_workdir(&workdir) {
            (self.log)(&format!("turn {}: {why}", row.id));
            return Closing::of(End::Unsafe, lines::UNSAFE.to_owned());
        }
        let shown = workdir.display().to_string();
        if let Err(e) = host::ensure(&self.client, db, row, &shown, agent.kind().name()) {
            let why = format!("the controller did not open a host Session: {e}");
            return Closing::of(End::Failed, lines::unavailable(&why));
        }
        let input = match self.context(db, row) {
            Ok(input) => input,
            Err(e) => {
                return Closing::of(End::Failed, lines::unavailable(&format!("slack.db: {e}")))
            }
        };
        let turn = Turn {
            setup: TurnSetup {
                model: &self.model,
                config_dir: &config_dir,
                workdir: &workdir,
                brief: BRIEF,
            },
            input: &input,
            credential: &credential,
            limits: LIMITS,
        };
        let mut spawned = |pid| self.written(db.set_pid(row.id, pid));
        let ended = turn::run(&*agent, &turn, &mut spawned, &*self.log);
        let mut closing = self.closing(ended, row, agent.kind().label(), &credential);
        closing.bytes = Some(input.len());
        closing
    }

    /// The end and line of a turn that ran.
    fn closing(&self, ended: Ended, row: &TurnRow, label: &str, credential: &Secret) -> Closing {
        let hide = |text: &str| lines::redact(text, credential);
        match ended {
            Ended::Answered(Answer::Reply { text, cost_usd }) => Closing {
                cost: cost_usd,
                ..Closing::of(End::Replied, lines::reply(&text))
            },
            Ended::Answered(Answer::Refused) => {
                Closing::of(End::TokenRefused, lines::refused(&row.owner, label))
            }
            Ended::Answered(Answer::Failed(line)) => {
                Closing::of(End::Failed, lines::stopped(&hide(&line)))
            }
            Ended::Answered(Answer::Unavailable(why)) => {
                Closing::of(End::Failed, lines::unavailable(&hide(&why)))
            }
            Ended::Unsafe(why) => {
                (self.log)(&format!("turn {}: {}", row.id, hide(&why)));
                Closing::of(End::Unsafe, lines::UNSAFE.to_owned())
            }
            Ended::Killed => Closing::of(End::Killed, lines::KILLED.to_owned()),
        }
    }

    /// The context of turn `row`, rebuilt from `slack.db`.
    fn context(&self, db: &Db, row: &TurnRow) -> rusqlite::Result<String> {
        let first = db.first_request(&row.channel, &row.thread_ts)?;
        let first_id = first.as_ref().map(|(id, _)| *id);
        let earlier: Vec<Said> = db
            .history(&row.channel, &row.thread_ts, row.id)?
            .into_iter()
            .filter(|(id, _)| Some(*id) != first_id)
            .map(|(_, said)| said)
            .collect();
        let new: Vec<Said> = db
            .turn_items(row.id)?
            .into_iter()
            .map(|(_, said)| said)
            .collect();
        let boxes: Vec<String> = db
            .thread_boxes(&row.channel, &row.thread_ts)?
            .iter()
            .map(|b| {
                let state = match &b.state {
                    BoxState::Starting => "starting".to_owned(),
                    BoxState::Bound {
                        branch: Some(branch),
                        ..
                    } => format!("running on branch {branch}"),
                    BoxState::Bound { branch: None, .. } => "running".to_owned(),
                    BoxState::Closing { .. } => "closing".to_owned(),
                    BoxState::Gone { .. } => "closed".to_owned(),
                };
                format!(
                    "- Box `{}`, asked for by <@{}>: {state}",
                    b.box_name.as_str(),
                    b.requested_by
                )
            })
            .collect();
        let parts = Parts {
            first_request: first.as_ref().map(|(_, text)| text.as_str()),
            actions: &[],
            boxes: &boxes,
            earlier: &earlier,
            new: &new,
        };
        Ok(context::render(&parts, self.budget))
    }
}

/// How a turn ended and the line that says so.
struct Closing {
    /// How it ended.
    end: End,
    /// What the thread is shown.
    line: String,
    /// Size of the context it ran with, when it ran.
    bytes: Option<usize>,
    /// Cost the agent reported.
    cost: Option<f64>,
}

impl Closing {
    /// A turn that ended as `end` with `line`, with nothing to log.
    const fn of(end: End, line: String) -> Self {
        Self {
            end,
            line,
            bytes: None,
            cost: None,
        }
    }
}

/// The thread's workdir, `<state dir>/threads/<channel>-<thread ts>`, created mode 0700 in a
/// private parent. A channel or ts that is not safe as a path component is refused.
fn workdir(state_dir: &Path, row: &TurnRow) -> Result<PathBuf, String> {
    let safe = |s: &str, extra: char| {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == extra)
    };
    if !safe(&row.channel, '_') || !safe(&row.thread_ts, '.') {
        return Err("the thread id is not safe as a directory name".to_owned());
    }
    let parent = std::path::absolute(state_dir.join("threads")).map_err(|e| e.to_string())?;
    let dir = parent.join(format!("{}-{}", row.channel, row.thread_ts));
    for d in [&parent, &dir] {
        private_dir(d, daemon_uid()).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    Ok(dir)
}
