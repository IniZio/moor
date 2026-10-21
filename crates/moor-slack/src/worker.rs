//! The start and close paths: a Box row in `starting` or `closing` in `slack.db` is a start or
//! close the front owes, which the coordinator's tool server writes from its own process. The
//! worker reads those rows every [`POLL`] and turns each into a started or closed Box over the
//! controller socket. The worker also posts replies, checks each repo a user adds to a channel,
//! and checks each Claude token a user pastes.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use moor_client::ClientError;
use moor_core::name::{BoxId, BoxName};
use moor_core::request::{Request, ShowReq, StartReq};
use moor_core::response::{ExitClass, Response};
use serde_json::Value;

use crate::command::{failed, started, BoxEvent};
use crate::db::{BoxState, Db, SlackBox};
use crate::dial::Backoff;
use crate::login::TokenCheck;
use crate::slash::check_repo;

/// How often the worker reads `slack.db` for Boxes to start or close.
const POLL: Duration = Duration::from_millis(500);

/// The worker's effects, so tests can script the controller and record what Slack sees.
pub(crate) struct Fx<'a> {
    /// Sends one request to the controller.
    pub(crate) call: &'a mut dyn FnMut(Request) -> Result<Response, ClientError>,
    /// Posts `(channel, thread, text)` in a thread.
    pub(crate) reply: &'a mut dyn FnMut(&str, &str, &str),
    /// Posts `(channel, text)` at the top level of a channel.
    pub(crate) say: &'a mut dyn FnMut(&str, &str),
    /// Checks a pasted Claude token, stores it when Claude takes it, and updates the modal.
    pub(crate) check_token: &'a mut dyn FnMut(TokenCheck),
    /// Writes one diagnostic line.
    pub(crate) log: &'a dyn Fn(&str),
}

/// What the worker does for one recorded mention or command.
#[derive(Debug, PartialEq, Eq)]
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
    /// Warn in `channel` when the controller does not know `repo`, which a user just added.
    CheckRepo {
        /// Channel id the repo was added to.
        channel: String,
        /// The repo as the user typed it.
        repo: String,
    },
    /// Check a pasted Claude token and store it when Claude takes it.
    CheckToken(TokenCheck),
}

/// Runs each job as it arrives, sends the first start or close of each new `starting` or
/// `closing` row every [`POLL`], and asks again about unsure starts and closes on a backoff
/// timer, so one the controller cannot confirm never holds up another thread. `left` holds
/// the rows an earlier run left `starting` or `closing`, read before this run sent anything;
/// they are asked about first.
pub(crate) fn serve(jobs: &Receiver<Job>, db: &Db, left: Vec<SlackBox>, fx: &mut Fx<'_>) {
    let mut unsure = Unsure::new(left);
    let mut poll = now();
    loop {
        if unsure.due.is_some_and(|due| due <= now()) {
            unsure.retry(db, fx);
            continue;
        }
        if poll <= now() {
            unsure.take_new(db, fx);
            poll = now() + POLL;
        }
        let wake = unsure.due.map_or(poll, |due| due.min(poll));
        match jobs.recv_timeout(wake.saturating_duration_since(now())) {
            Ok(Job::Reply {
                channel,
                thread,
                text,
            }) => (fx.reply)(&channel, &thread, &text),
            Ok(Job::CheckRepo { channel, repo }) => check_repo(fx, &channel, &repo),
            Ok(Job::CheckToken(check)) => (fx.check_token)(check),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// The current instant; `recv_timeout` waits on real time, so retry deadlines do too.
#[allow(
    clippy::disallowed_methods,
    reason = "the worker waits with recv_timeout, which runs on real time"
)]
fn now() -> Instant {
    Instant::now()
}

/// Starts and closes the controller has not confirmed, asked about again on one timer.
struct Unsure {
    /// The rows, oldest first.
    rows: Vec<SlackBox>,
    /// Wait after a round that leaves rows unsure; it grows until a round settles them all.
    backoff: Backoff,
    /// When the next round runs; `None` while no row is unsure.
    due: Option<Instant>,
}

impl Unsure {
    /// Rows an earlier run left `starting` or `closing`, due at once.
    fn new(rows: Vec<SlackBox>) -> Self {
        let due = (!rows.is_empty()).then(now);
        Self {
            rows,
            backoff: Backoff::new(),
            due,
        }
    }

    /// Sends the first start or close of each `starting` or `closing` row that is not already
    /// unsure. Only this worker sends them, so such a row has had none sent yet.
    fn take_new(&mut self, db: &Db, fx: &mut Fx<'_>) {
        let rows = db.starting().and_then(|mut rows| {
            rows.extend(db.closing()?);
            Ok(rows)
        });
        let rows = match rows {
            Ok(rows) => rows,
            Err(e) => return (fx.log)(&format!("slack.db: {e}; reading again")),
        };
        for row in rows {
            if self.rows.iter().any(|r| r.box_name == row.box_name) {
                continue;
            }
            if !settle(db, fx, &row, Attempt::First) {
                self.add(row);
            }
        }
    }

    /// Adds a row whose first start or close was not confirmed.
    fn add(&mut self, row: SlackBox) {
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

/// Whether a `start` or `close` for the row may already have reached the controller.
#[derive(Clone, Copy)]
enum Attempt {
    /// Nothing was sent yet, so an error frame proves the request never ran.
    First,
    /// A start may have run. `start` attaches only to an open Box, so a blind resend after
    /// the first Box closed would make a second Box with the same name; `show` goes first.
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

/// Tries once to settle the start or close that `row`'s state says is pending. Returns
/// whether the row is settled.
fn settle(db: &Db, fx: &mut Fx<'_>, row: &SlackBox, attempt: Attempt) -> bool {
    match row.state {
        BoxState::Starting => settle_start(db, fx, row, attempt),
        BoxState::Closing { .. } => close::settle_close(db, fx, row, attempt),
        BoxState::Bound { .. } | BoxState::Gone { .. } => true,
    }
}

/// Tries once to settle the start of `row`: binds or deletes it, or logs why it stays
/// `starting`. Returns whether the row is settled.
fn settle_start(db: &Db, fx: &mut Fx<'_>, row: &SlackBox, attempt: Attempt) -> bool {
    match ask(fx, row, attempt) {
        Outcome::Bound(box_id, branch) => bind(db, fx, row, box_id, branch.as_deref()),
        Outcome::Refused(message) => refuse(db, fx, row, &message),
        Outcome::Unsure(why) => {
            let name = row.box_name.as_str();
            (fx.log)(&format!("start of {name}: {why}; asking again"));
            false
        }
    }
}

/// Sends at most one `show` and one `start` for `row`, so each attempt ends within the
/// client's timeouts.
fn ask(fx: &mut Fx<'_>, row: &SlackBox, attempt: Attempt) -> Outcome {
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
fn start_request(row: &SlackBox) -> Request {
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
        parent: row.parent,
    })
}

/// The `show` request for `row`'s Box.
fn show_request(row: &SlackBox) -> Request {
    Request::Show(ShowReq {
        target: Some(row.box_name.clone().into()),
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

/// Says in the thread that the Box started, then binds the row to it and queues the notice in
/// one transaction. Posting first means a crash in between posts again on restart instead of
/// losing the reply. Returns whether the row is bound; a row left `starting` is asked about
/// again.
fn bind(db: &Db, fx: &mut Fx<'_>, row: &SlackBox, box_id: BoxId, branch: Option<&str>) -> bool {
    let line = started(&row.box_name, branch);
    (fx.reply)(&row.channel, &row.thread_ts, &line);
    written(
        fx,
        db.atomically(|| {
            db.bind(&row.box_name, box_id, branch)?;
            db.notice(row, BoxEvent::Started, None).map(drop)
        }),
    )
}

/// Posts why the controller refused the start, then deletes the row and queues the notice,
/// posting first for the same reason as [`bind`]. Returns whether the row is gone.
fn refuse(db: &Db, fx: &mut Fx<'_>, row: &SlackBox, message: &str) -> bool {
    (fx.reply)(
        &row.channel,
        &row.thread_ts,
        &failed(&row.box_name, message),
    );
    written(
        fx,
        db.atomically(|| {
            db.forget_starting(&row.box_name)?;
            db.notice(row, BoxEvent::NotStarted, None).map(drop)
        }),
    )
}

/// Whether a `slack.db` write succeeded, logging the error when it did not.
fn written(fx: &Fx<'_>, write: rusqlite::Result<()>) -> bool {
    write
        .inspect_err(|e| (fx.log)(&format!("slack.db: {e}; asking again")))
        .is_ok()
}

mod close;
mod intake;
pub(crate) use intake::record;

#[cfg(test)]
mod tests;
