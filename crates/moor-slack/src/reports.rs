//! Workflow reports: one poller per person who added a workflow here reads their
//! `workflow-runs` stream and tells the thread that added each workflow what its runs did. A
//! run with a Box gets a thread of its own in the same channel, which mentions the owner and
//! links the thread that added the workflow. The Box is adopted into it, so the feed shows its
//! status, artifacts and prompts there and queues the agent's final message when it finishes.
//! The reporter itself posts only what the feed does not: a run that timed out, one skipped
//! or refused, and a workflow the controller disabled.
//!
//! Each change is handled in one `slack.db` transaction with the cursor past it, so a restart
//! repeats nothing and skips nothing. A line to post is first written to an outbox in that
//! transaction and then posted, so a failed post never holds the cursor: it is tried again on
//! the next polls, and given up on, with a log line, after [`GIVE_UP`] failures.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_client::ClientError;
use moor_core::request::{Request, WorkflowRunsReq};
use moor_core::response::{ExitClass, Response};
use moor_core::secret::Secret;
use moor_core::view::workflow::{ChangeView, LastRunView, WorkflowRunsValue};

use crate::api::Api;
use crate::db::{Db, Outbox};
use crate::error_text::error_text;
use crate::escape::{code, escape};
use crate::feed::Feed;
use crate::identity::{Link, NAMESPACE};
use crate::Log;

/// How often each person's runs are read.
const POLL: Duration = Duration::from_secs(5);

/// Failed posts of one line after which it is dropped.
const GIVE_UP: i64 = 5;

/// Changes read at once.
const BATCH: u32 = 200;

/// Posts `(channel, thread, text)` and returns the message `ts`.
pub(crate) type Poster<'a> = &'a mut dyn FnMut(&str, Option<&str>, &str) -> Result<String, String>;

/// What one poll acts through, so tests can script the controller and record what Slack sees.
pub(crate) struct Fx<'a> {
    /// Sends one request to the controller as the person.
    pub(crate) call: &'a mut dyn FnMut(Request) -> Result<Response, ClientError>,
    /// Posts `(channel, thread, text)`, as a new top-level message without a thread; returns
    /// the message `ts`.
    pub(crate) post: Poster<'a>,
    /// Writes one diagnostic line.
    pub(crate) log: &'a dyn Fn(&str),
    /// The current time in unix seconds.
    pub(crate) clock: fn() -> i64,
}

/// The reporters' shared state: where `slack.db` lives, Slack and the controller.
pub(crate) struct Reports {
    /// The controller and team.
    pub(crate) link: Link,
    /// Path of `slack.db`; each reporter opens its own connection.
    pub(crate) db_path: PathBuf,
    /// Web API client.
    pub(crate) api: Arc<Api>,
    /// Bot token for posting.
    pub(crate) bot_token: Arc<Secret>,
    /// Daemon log.
    pub(crate) log: Log,
    /// The current time in unix seconds.
    pub(crate) clock: fn() -> i64,
}

impl Reports {
    /// Reporters that reach Slack and the controller the way `feed` does.
    pub(crate) fn beside(feed: &Feed) -> Self {
        Self {
            link: feed.link.clone(),
            db_path: feed.db_path.clone(),
            api: Arc::clone(&feed.api),
            bot_token: Arc::clone(&feed.bot_token),
            log: Arc::clone(&feed.log),
            clock: feed.clock,
        }
    }

    /// Starts a reporter for each thread owner that has none, until the process ends.
    pub(crate) fn run(&self) {
        let db = match Db::open(&self.db_path) {
            Ok(db) => db,
            Err(e) => return (self.log)(&format!("open slack.db: {e}")),
        };
        let mut started = HashSet::new();
        loop {
            match db.workflow_owners() {
                Ok(owners) => {
                    for owner in owners {
                        if started.insert(owner.clone()) {
                            self.start(owner);
                        }
                    }
                }
                Err(e) => (self.log)(&format!("slack.db: {e}; reading again")),
            }
            thread::sleep(POLL);
        }
    }

    /// Starts `owner`'s reporter; an owner with no valid identity gets none.
    fn start(&self, owner: String) {
        let identity = match self.link.identity(&owner) {
            Ok(identity) => identity,
            Err(why) => return (self.log)(&format!("workflows: no reports for {owner}: {why}")),
        };
        let client = self.link.client_as(identity);
        let (api, bot) = (Arc::clone(&self.api), Arc::clone(&self.bot_token));
        let (log, db_path, clock) = (Arc::clone(&self.log), self.db_path.clone(), self.clock);
        thread::spawn(move || {
            let db = match Db::open(&db_path) {
                Ok(db) => db,
                Err(e) => return log(&format!("open slack.db: {e}")),
            };
            let mut call = |request| client.request(request, |_| {});
            let mut post = |channel: &str, thread: Option<&str>, text: &str| {
                thread
                    .map_or_else(
                        || api.post_top_level(&bot, channel, text),
                        |thread| api.post_message(&bot, channel, thread, text),
                    )
                    .map_err(|e| e.to_string())
            };
            let mut fx = Fx {
                call: &mut call,
                post: &mut post,
                log: &*log,
                clock,
            };
            let mut said = None;
            loop {
                let polled = poll(&db, &owner, &mut fx);
                if let Err(why) = &polled {
                    if said.as_ref() != Some(why) {
                        log(&format!("workflows: {owner}: {why}"));
                    }
                }
                said = polled.err();
                thread::sleep(POLL);
            }
        });
    }
}

/// Handles `owner`'s changes after their cursor, then posts the lines owed. An error says why
/// the controller or `slack.db` could not be read; the cursor then stays where it was.
pub(crate) fn poll(db: &Db, owner: &str, fx: &mut Fx<'_>) -> Result<(), String> {
    let handled = changes(db, owner, fx);
    open_threads(db, owner, fx);
    flush(db, owner, fx);
    handled
}

/// Handles each change after the cursor, oldest first, stopping at the first that cannot be.
fn changes(db: &Db, owner: &str, fx: &mut Fx<'_>) -> Result<(), String> {
    let after = db
        .workflow_cursor(owner)
        .map_err(|e| format!("slack.db: {e}"))?;
    let request = Request::WorkflowRuns(WorkflowRunsReq {
        identity: None,
        after: Some(after),
        limit: Some(BATCH),
    });
    let reply = (fx.call)(request).map_err(|e| format!("controller: {e}"))?;
    if reply.class != ExitClass::Ok {
        return Err(format!("controller: {}", error_text(reply)));
    }
    let runs: WorkflowRunsValue = serde_json::from_value(reply.value.unwrap_or_default())
        .map_err(|e| format!("controller sent runs this build cannot read: {e}"))?;
    for change in runs.changes {
        handle(db, owner, &change, fx)?;
    }
    Ok(())
}

/// What a change asks of the front.
enum Say {
    /// Nothing to post.
    Nothing,
    /// A line for the thread, and the streak it belongs to, if any.
    Line(String, Option<&'static str>),
}

/// Handles one change in one transaction with the cursor past it. Only a change of a workflow
/// this front added is acted on.
fn handle(db: &Db, owner: &str, change: &ChangeView, fx: &mut Fx<'_>) -> Result<(), String> {
    let (seq, workflow, origin) = match change {
        ChangeView::Run {
            seq,
            workflow,
            origin,
            ..
        }
        | ChangeView::Disabled {
            seq,
            workflow,
            origin,
            ..
        } => (*seq, workflow, origin),
    };
    let thread = origin
        .as_ref()
        .filter(|o| o.front == NAMESPACE)
        .and_then(|o| o.key.as_deref()?.split_once('/'));
    let Some((channel, thread_ts)) = thread else {
        return advance(db, owner, seq);
    };
    let known = db
        .thread(channel, thread_ts)
        .map_err(|e| format!("slack.db: {e}"))?;
    if known.is_none() {
        (fx.log)(&format!("workflows: {workflow} reports to thread {channel}/{thread_ts}, which this front does not know"));
        return advance(db, owner, seq);
    }
    let (owe, said) = match change {
        ChangeView::Run { run, .. } => (
            run_thread(db, owner, workflow, (channel, thread_ts, run), fx)?,
            run_line(workflow, run),
        ),
        ChangeView::Disabled { reason, .. } => (None, disabled_line(workflow, reason)),
    };
    let owed = (channel, thread_ts, said);
    db.atomically(|| {
        match &owe {
            Some(RunStep::Owe(thread)) => db.owe_run_thread(thread)?,
            Some(RunStep::Learn(name, id, branch)) => {
                db.learn_run_box(name, (*id, branch.as_deref()), (fx.clock)())?;
            }
            None => {}
        }
        record(db, owner, change, &owed)?;
        db.set_workflow_cursor(owner, seq)
    })
    .map_err(|e| format!("slack.db: {e}"))
}

/// Writes the line `change` owes its thread, unless it repeats a streak already told, and
/// ends the streak of a run that went another way.
fn record(
    db: &Db,
    owner: &str,
    change: &ChangeView,
    (channel, thread_ts, said): &(&str, &str, Say),
) -> rusqlite::Result<()> {
    let (line, kind) = match said {
        Say::Line(line, kind) => (Some(line), *kind),
        Say::Nothing => (None, None),
    };
    let ChangeView::Run { workflow, run, .. } = change else {
        let at = (*channel, *thread_ts);
        return line.map_or(Ok(()), |l| db.enqueue_line(owner, at, None, l));
    };
    if let Some(planned) = run
        .planned_box
        .as_deref()
        .filter(|_| run.outcome == "refused")
    {
        if db.run_thread_known(planned)? {
            let line = refused_in_thread(workflow, run);
            return db.enqueue_line(owner, (channel, thread_ts), Some(planned), &line);
        }
    }
    let timed_out = run.outcome == "timed_out";
    let boxed = run.box_name.as_deref().filter(|_| timed_out);
    let fresh = if let Some(kind) = kind {
        db.note_streak(owner, (workflow, &run.repo, kind))?
    } else {
        db.clear_streak(owner, workflow, &run.repo)?;
        true
    };
    match line {
        Some(line) if fresh => db.enqueue_line(owner, (channel, thread_ts), boxed, line),
        _ => Ok(()),
    }
}

/// Moves the cursor past `seq`.
fn advance(db: &Db, owner: &str, seq: i64) -> Result<(), String> {
    db.set_workflow_cursor(owner, seq)
        .map_err(|e| format!("slack.db: {e}"))
}

/// What the thread is told of `run`; a started or finished run says nothing, since the feed
/// shows the adopted Box and its agent's final message.
fn run_line(workflow: &str, run: &LastRunView) -> Say {
    let (wf, repo) = (code(workflow), code(&run.repo));
    let boxed = run.box_name.as_deref().map(code).unwrap_or_default();
    match run.outcome.as_str() {
        "timed_out" => Say::Line(
            format!("Workflow {wf} ran out of time on {repo}. Box {boxed} was stopped at its limit and is still open. Ask me to show or close it."),
            None,
        ),
        "skipped_overlap" => Say::Line(
            format!("Workflow {wf} skipped {repo}: the Box of its previous run is still open. Ask me to close that Box so the next run can start. I will not say this again until a run goes ahead."),
            Some("skipped"),
        ),
        "refused" => Say::Line(
            format!("Workflow {wf} could not start on {repo}: {}. Fix that, or ask me to disable the workflow. I will not say this again until a run goes ahead.", escape(run.detail.as_deref().unwrap_or("no reason given"))),
            Some("refused"),
        ),
        _ => Say::Nothing,
    }
}

/// The line for a run that fired a thread of its own and then could not start; it is that
/// run's only report, so it is never held back as a repeat.
fn refused_in_thread(workflow: &str, run: &LastRunView) -> String {
    format!(
        "Workflow {} could not start on {}: {}. Fix that, or ask me to disable the workflow.",
        code(workflow),
        code(&run.repo),
        escape(run.detail.as_deref().unwrap_or("no reason given"))
    )
}

/// The line for a workflow the controller disabled.
fn disabled_line(workflow: &str, reason: &str) -> Say {
    Say::Line(
        format!(
            "Workflow {} was disabled: {}. Ask me to enable it once that is fixed.",
            code(workflow),
            escape(reason)
        ),
        None,
    )
}

/// Posts `owner`'s owed lines, oldest first. A line that fails stays for the next poll and
/// is dropped after [`GIVE_UP`] failures; the others are unaffected.
fn flush(db: &Db, owner: &str, fx: &mut Fx<'_>) {
    let owed = match db.owed_lines(owner) {
        Ok(owed) => owed,
        Err(e) => return (fx.log)(&format!("slack.db: {e}; reading again")),
    };
    for line in owed {
        let at = match line_thread(db, &line) {
            Ok(Some(at)) => at,
            Ok(None) => continue,
            Err(e) => {
                (fx.log)(&format!("slack.db: {e}"));
                continue;
            }
        };
        let recorded = match (fx.post)(&at.0, Some(&at.1), &line.text) {
            Ok(_) => db.line_done(line.id),
            Err(why) if line.attempts + 1 >= GIVE_UP => {
                (fx.log)(&format!(
                    "slack: workflow line to {}/{} dropped: {why}",
                    at.0, at.1
                ));
                db.line_done(line.id)
            }
            Err(why) => {
                (fx.log)(&format!("slack: workflow line failed: {why}"));
                db.line_failed(line.id)
            }
        };
        if let Err(e) = recorded {
            (fx.log)(&format!("slack.db: {e}"));
        }
    }
}

/// Where `line` is posted: its run's thread when it has one, else the thread that added the
/// workflow. None while the Box's thread is still owed, so the line waits for it.
fn line_thread(db: &Db, line: &Outbox) -> rusqlite::Result<Option<(String, String)>> {
    let Some(name) = &line.box_name else {
        return Ok(Some((line.channel.clone(), line.thread_ts.clone())));
    };
    if let Some(at) = db.run_thread_at(name)?.or(db.box_thread(name)?) {
        return Ok(Some(at));
    }
    if db.run_thread_pending(name, GIVE_UP)? {
        return Ok(None);
    }
    Ok(Some((line.channel.clone(), line.thread_ts.clone())))
}

mod threads;
use threads::{open_threads, run_thread, RunStep};

#[cfg(test)]
mod tests;
