//! The start path: the socket thread records each mention in `slack.db` before Slack gets
//! its ack, and the worker turns each recorded start into a Box over the controller socket.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Instant;

use moor_client::ClientError;
use moor_core::name::{BoxId, BoxName};
use moor_core::request::{Request, ShowReq, StartReq};
use moor_core::response::{ExitClass, Response};
use serde_json::Value;

use crate::command::{decide, failed, started, Action, Allow};
use crate::db::{Db, SlackThread, ThreadState};
use crate::dial::Backoff;
use crate::socket::Mention;

/// The worker's effects, so tests can script the controller and record what Slack sees.
pub(crate) struct Fx<'a> {
    /// Sends one request to the controller.
    pub(crate) call: &'a mut dyn FnMut(Request) -> Result<Response, ClientError>,
    /// Posts `(channel, thread, text)` in a thread.
    pub(crate) reply: &'a mut dyn FnMut(&str, &str, &str),
    /// Writes one diagnostic line.
    pub(crate) log: &'a dyn Fn(&str),
}

/// What the worker does for one recorded mention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Job {
    /// Post `text` in a thread.
    Reply {
        /// Channel id of the thread.
        channel: String,
        /// Timestamp of the thread's root message.
        thread: String,
        /// What to post.
        text: String,
    },
    /// Send the first `start` for this row, already `starting` in `slack.db`.
    Start(SlackThread),
}

/// Records one mention in a single transaction: its event id and, for an allowed start, the
/// thread's `starting` row with the goal, repo and owner. Returns what the worker does next,
/// or nothing for a redelivered event.
pub(crate) fn record(
    mention: &Mention,
    db: &Db,
    allow: &Allow<'_>,
    now: i64,
) -> rusqlite::Result<Option<Job>> {
    db.atomically(|| {
        if !db.event_once(&mention.event_id, now)? || mention.reply {
            return Ok(None);
        }
        let channel = mention.channel.as_str();
        let thread = mention.thread_ts.as_deref().unwrap_or(&mention.ts);
        let row = db.thread(channel, thread)?;
        let bound = row.as_ref().map(|t| &t.box_name);
        Ok(
            match decide(allow, channel, &mention.user, &mention.text, thread, bound) {
                Action::Ignore => None,
                Action::Reply(text) => Some(Job::Reply {
                    channel: channel.to_owned(),
                    thread: thread.to_owned(),
                    text,
                }),
                Action::Start { name, repo, goal } => {
                    let row = SlackThread {
                        channel: channel.to_owned(),
                        thread_ts: thread.to_owned(),
                        box_name: name,
                        repo,
                        goal,
                        owner: mention.user.clone(),
                        state: ThreadState::Starting,
                    };
                    db.insert_starting(&row, now)?;
                    Some(Job::Start(row))
                }
            },
        )
    })
}

/// Runs each job as it arrives and asks again about unsure starts on a backoff timer, so a
/// start the controller cannot confirm never holds up another thread. `left` holds the rows
/// an earlier run left `starting`, read before any mention was recorded, so no job names
/// them; they are asked about first.
pub(crate) fn serve(jobs: &Receiver<Job>, db: &Db, left: Vec<SlackThread>, fx: &mut Fx<'_>) {
    let mut unsure = Unsure::new(left);
    loop {
        if unsure.due.is_some_and(|due| due <= now()) {
            unsure.retry(db, fx);
            continue;
        }
        match next_job(jobs, unsure.due) {
            Ok(Job::Reply {
                channel,
                thread,
                text,
            }) => (fx.reply)(&channel, &thread, &text),
            Ok(Job::Start(row)) => {
                if !settle(db, fx, &row, Attempt::First) {
                    unsure.add(row);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// The next job, waiting no later than `due` when it is set.
fn next_job(jobs: &Receiver<Job>, due: Option<Instant>) -> Result<Job, RecvTimeoutError> {
    due.map_or_else(
        || jobs.recv().map_err(|_| RecvTimeoutError::Disconnected),
        |due| jobs.recv_timeout(due.saturating_duration_since(now())),
    )
}

/// The current instant; `recv_timeout` waits on real time, so retry deadlines do too.
#[allow(
    clippy::disallowed_methods,
    reason = "the worker waits with recv_timeout, which runs on real time"
)]
fn now() -> Instant {
    Instant::now()
}

/// Starts the controller has not confirmed, asked about again together on one timer.
struct Unsure {
    /// The rows, oldest first.
    rows: Vec<SlackThread>,
    /// Wait after a round that leaves rows unsure; it grows until a round settles them all.
    backoff: Backoff,
    /// When the next round runs; `None` while no row is unsure.
    due: Option<Instant>,
}

impl Unsure {
    /// Rows an earlier run left `starting`, due at once.
    fn new(rows: Vec<SlackThread>) -> Self {
        let due = (!rows.is_empty()).then(now);
        Self {
            rows,
            backoff: Backoff::new(),
            due,
        }
    }

    /// Adds a row whose first start was not confirmed.
    fn add(&mut self, row: SlackThread) {
        self.rows.push(row);
        if self.due.is_none() {
            self.due = Some(now() + self.backoff.next());
        }
    }

    /// Asks once about each row and keeps the ones still unsure.
    fn retry(&mut self, db: &Db, fx: &mut Fx<'_>) {
        self.rows.retain(|row| !settle(db, fx, row, Attempt::Again));
        self.due = if self.rows.is_empty() {
            self.backoff = Backoff::new();
            None
        } else {
            Some(now() + self.backoff.next())
        };
    }
}

/// Whether a `start` for the row may already have reached the controller.
#[derive(Clone, Copy)]
enum Attempt {
    /// Nothing was sent yet, so an error frame proves the start never ran.
    First,
    /// A start may have run. `start` attaches only to an open Box, so a blind resend after
    /// the first Box closed would make a second Box with the same name: ask `show` first.
    /// No client error can prove that nothing runs, so any error leaves the row unsure.
    Again,
}

/// How one attempt at a start ends.
enum Outcome {
    /// The controller named this Box, with its id and branch.
    Bound(BoxId, Option<String>),
    /// No Box runs for the thread; the text says why.
    Refused(String),
    /// A start may have run and the controller cannot confirm it now; the text says why.
    Unsure(String),
}

/// What a `show` reply says about the thread's Box.
enum Shown {
    /// The controller has no Box with that name.
    Absent,
    /// The reply settles the start.
    Settled(Outcome),
}

/// Tries once to settle `row`: binds or deletes it, or logs why it stays `starting`.
/// Returns whether the row is settled.
fn settle(db: &Db, fx: &mut Fx<'_>, row: &SlackThread, attempt: Attempt) -> bool {
    match ask(fx, row, attempt) {
        Outcome::Bound(box_id, branch) => {
            bind(db, fx, row, box_id, branch.as_deref());
            true
        }
        Outcome::Refused(message) => {
            refuse(db, fx, row, &message);
            true
        }
        Outcome::Unsure(why) => {
            let name = row.box_name.as_str();
            (fx.log)(&format!("start of {name}: {why}; asking again"));
            false
        }
    }
}

/// Sends at most one `show` and one `start` for `row`, so each attempt ends within the
/// client's timeouts.
fn ask(fx: &mut Fx<'_>, row: &SlackThread, attempt: Attempt) -> Outcome {
    if matches!(attempt, Attempt::Again) {
        match (fx.call)(show_request(row)).map(|reply| shown(reply, &row.box_name)) {
            Ok(Shown::Absent) => {}
            Ok(Shown::Settled(outcome)) => return outcome,
            Err(e) => return Outcome::Unsure(e.to_string()),
        }
    }
    match (fx.call)(start_request(row)) {
        Ok(reply) => start_outcome(reply),
        Err(e) if matches!(attempt, Attempt::First) && !lost(&e) => Outcome::Refused(e.to_string()),
        Err(e) => Outcome::Unsure(e.to_string()),
    }
}

/// The `start` request for `row`.
fn start_request(row: &SlackThread) -> Request {
    Request::Start(StartReq {
        name: row.box_name.clone(),
        goal: Some(row.goal.clone()),
        issue: None,
        worktree: None,
        base: None,
        model: None,
        no_agent: false,
        hold: false,
        resume: false,
        trust_moor_yaml: false,
        repo: Some(row.repo.clone()),
        parent: None,
    })
}

/// The `show` request for `row`'s Box.
fn show_request(row: &SlackThread) -> Request {
    Request::Show(ShowReq {
        box_name: Some(row.box_name.clone()),
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    })
}

/// The outcome of a `start` reply: its Box, or the controller's refusal.
fn start_outcome(reply: Response) -> Outcome {
    if reply.class != ExitClass::Ok {
        return Outcome::Refused(error_text(reply));
    }
    named_box(reply.value.as_ref()).map_or_else(
        || Outcome::Unsure("the start reply names no Box".to_owned()),
        Outcome::from,
    )
}

/// What a `show` reply says. Only the controller's `no Box <name>` refusal means absent; a
/// Box whose preflight rejected it never ran, so it counts as a refused start.
fn shown(reply: Response, name: &BoxName) -> Shown {
    if reply.class != ExitClass::Ok {
        let message = error_text(reply);
        return if message == format!("no Box {}", name.as_str()) {
            Shown::Absent
        } else {
            Shown::Settled(Outcome::Unsure(message))
        };
    }
    let boxed = reply.value.as_ref().and_then(|v| v.get("box"));
    if boxed
        .and_then(|b| b.get("lifecycle"))
        .and_then(Value::as_str)
        == Some("rejected")
    {
        let reason = boxed
            .and_then(|b| b.get("state_reason"))
            .and_then(Value::as_str);
        return Shown::Settled(Outcome::Refused(reason.unwrap_or("rejected").to_owned()));
    }
    Shown::Settled(named_box(reply.value.as_ref()).map_or_else(
        || Outcome::Unsure("the show reply names no Box".to_owned()),
        Outcome::from,
    ))
}

impl From<(BoxId, Option<String>)> for Outcome {
    fn from((box_id, branch): (BoxId, Option<String>)) -> Self {
        Self::Bound(box_id, branch)
    }
}

/// The id and branch of the Box under `value.box`.
fn named_box(value: Option<&Value>) -> Option<(BoxId, Option<String>)> {
    let boxed = value?.get("box")?;
    let id = boxed.get("id").and_then(Value::as_i64)?;
    let branch = boxed.get("branch").and_then(Value::as_str);
    Some((BoxId::new(id), branch.map(str::to_owned)))
}

/// The error text of a failed reply.
fn error_text(reply: Response) -> String {
    reply
        .error
        .map_or_else(|| format!("{} failed", reply.cmd), |e| e.message)
}

/// True when the request may have reached the controller, or the controller may answer
/// later, so the worker must ask before it acts again. Only an error frame proves the
/// request never ran; the controller also refuses a hello with one.
const fn lost(e: &ClientError) -> bool {
    !matches!(e, ClientError::Frame { .. })
}

/// Binds the row to the Box the controller named and says so in the thread.
fn bind(db: &Db, fx: &mut Fx<'_>, row: &SlackThread, box_id: BoxId, branch: Option<&str>) {
    if let Err(e) = db.bind(&row.channel, &row.thread_ts, box_id, branch) {
        (fx.log)(&format!("slack.db: {e}"));
    }
    (fx.reply)(&row.channel, &row.thread_ts, &started(&row.box_name));
}

/// Deletes the row of a start the controller refused and posts why.
fn refuse(db: &Db, fx: &mut Fx<'_>, row: &SlackThread, message: &str) {
    if let Err(e) = db.forget_starting(&row.channel, &row.thread_ts) {
        (fx.log)(&format!("slack.db: {e}"));
    }
    (fx.reply)(
        &row.channel,
        &row.thread_ts,
        &failed(&row.box_name, message),
    );
}

#[cfg(test)]
mod tests;
