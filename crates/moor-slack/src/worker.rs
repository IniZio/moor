//! The worker: turns an allowed mention into a Box over the controller socket and binds the
//! thread to it.

use std::time::Duration;

use moor_client::ClientError;
use moor_core::name::{BoxId, BoxName};
use moor_core::request::{Request, ShowReq, StartReq};
use moor_core::response::{ExitClass, Response};
use serde_json::Value;

use crate::command::{decide, failed, started, Action, Allow};
use crate::db::{Db, SlackThread, ThreadState};
use crate::dial::Backoff;
use crate::socket::Mention;

/// How moor-client words a hello that the controller answered with another protocol or frame.
const HELLO_REFUSED: [&str; 2] = ["controller speaks protocol", "expected hello frame"];

/// The worker's effects, so tests can script the controller and record what Slack sees.
pub(crate) struct Fx<'a> {
    /// Sends one request to the controller.
    pub(crate) call: &'a mut dyn FnMut(Request) -> Result<Response, ClientError>,
    /// Posts `(channel, thread, text)` in a thread.
    pub(crate) reply: &'a mut dyn FnMut(&str, &str, &str),
    /// Writes one diagnostic line.
    pub(crate) log: &'a dyn Fn(&str),
    /// Waits before the controller is asked again.
    pub(crate) wait: &'a dyn Fn(Duration),
    /// The current time in unix seconds.
    pub(crate) now: fn() -> i64,
}

/// The request a start sends next.
#[derive(Clone, Copy)]
enum Step {
    /// Send `start`.
    Start,
    /// Ask `show` whether the Box exists.
    Show,
}

/// Applies one mention: dedupe, then reply, or start a Box and bind the thread to it.
pub(crate) fn handle(mention: &Mention, db: &Db, allow: &Allow<'_>, fx: &mut Fx<'_>) {
    match db.event_once(&mention.event_id, (fx.now)()) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => return (fx.log)(&format!("slack.db: {e}")),
    }
    if mention.reply {
        return;
    }
    let channel = mention.channel.as_str();
    let thread = mention.thread_ts.as_deref().unwrap_or(&mention.ts);
    let row = match db.thread(channel, thread) {
        Ok(row) => row,
        Err(e) => return (fx.log)(&format!("slack.db: {e}")),
    };
    let bound = row.as_ref().map(|t| &t.box_name);
    match decide(allow, channel, &mention.user, &mention.text, thread, bound) {
        Action::Ignore => {}
        Action::Reply(text) => (fx.reply)(channel, thread, &text),
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
            match db.insert_starting(&row, (fx.now)()) {
                Ok(()) => settle(db, fx, &row, Step::Start),
                Err(e) => (fx.log)(&format!("slack.db: {e}")),
            }
        }
    }
}

/// Settles every row left in `starting` by an earlier run, oldest first.
pub(crate) fn resume(db: &Db, fx: &mut Fx<'_>) {
    match db.starting() {
        Ok(rows) => rows.iter().for_each(|row| settle(db, fx, row, Step::Show)),
        Err(e) => (fx.log)(&format!("slack.db: {e}")),
    }
}

/// How the start of one thread ends.
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

/// Most `start` requests one row may lose before the worker leaves it for the next startup.
const MAX_LOST_STARTS: u32 = 3;

/// Moves a `starting` row to bound or deleted, or leaves it `starting` when unsure.
fn settle(db: &Db, fx: &mut Fx<'_>, row: &SlackThread, step: Step) {
    match drive(fx, row, step) {
        Outcome::Bound(box_id, branch) => bind(db, fx, row, box_id, branch.as_deref()),
        Outcome::Refused(message) => refuse(db, fx, row, &message),
        Outcome::Unsure(why) => {
            let name = row.box_name.as_str();
            (fx.log)(&format!(
                "start of {name}: {why}; checked again at the next startup"
            ));
        }
    }
}

/// Sends requests until the start of `row` is settled.
///
/// After a lost connection the worker waits with backoff and asks `show` first: `start`
/// attaches only to an open Box, so a blind resend after the first Box closed would make
/// a second Box with the same name. Once a start may have reached the controller, no client
/// error can prove that nothing runs, so such an error leaves the row unsure.
fn drive(fx: &mut Fx<'_>, row: &SlackThread, mut step: Step) -> Outcome {
    let mut backoff = Backoff::new();
    let mut lost_starts = 0;
    let mut sure = matches!(step, Step::Start);
    loop {
        let e = match ((fx.call)(request(row, step)), step) {
            (Ok(reply), Step::Start) => return start_outcome(reply),
            (Ok(reply), Step::Show) => match shown(reply, &row.box_name) {
                Shown::Absent => {
                    step = Step::Start;
                    continue;
                }
                Shown::Settled(outcome) => return outcome,
            },
            (Err(e), _) if !lost(&e) && sure => return Outcome::Refused(e.to_string()),
            (Err(e), _) if !lost(&e) => return Outcome::Unsure(e.to_string()),
            (Err(e), _) => e,
        };
        if matches!(step, Step::Start) {
            sure = false;
            lost_starts += 1;
            if lost_starts >= MAX_LOST_STARTS {
                return Outcome::Unsure(e.to_string());
            }
        }
        (fx.log)(&format!(
            "start of {}: {e}; asking again",
            row.box_name.as_str()
        ));
        (fx.wait)(backoff.next());
        step = Step::Show;
    }
}

/// The request for `step` of `row`'s start.
fn request(row: &SlackThread, step: Step) -> Request {
    match step {
        Step::Start => Request::Start(StartReq {
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
        }),
        Step::Show => Request::Show(ShowReq {
            box_name: Some(row.box_name.clone()),
            events: None,
            agent: false,
            reports: false,
            artifacts: false,
            pane: false,
        }),
    }
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
/// later, so the worker must ask before it acts again. Only an error frame or a refused
/// hello proves the request never ran.
fn lost(e: &ClientError) -> bool {
    match e {
        ClientError::Connect { .. } | ClientError::Io(_) | ClientError::Timeout => true,
        ClientError::Protocol(message) => !HELLO_REFUSED.iter().any(|p| message.starts_with(p)),
        ClientError::Frame { .. } => false,
    }
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
