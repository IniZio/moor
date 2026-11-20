//! A pull request event from the App's webhook, for every workflow whose first repo it is on.
//!
//! The payload only names the pull request. What fires is decided on the pull request as GitHub
//! has it now, read back as the App, so a late delivery of an older head never cancels a newer
//! one. An open pull request that is no draft, no bot's, and no fork of an author who cannot
//! write fires each enabled workflow that has `on_pull_request`, through the claim a manual run
//! for a pull request uses. A newer head cancels the open run of an older one, and the same head
//! again records nothing. Such a fork records `skipped_fork` with its author. A closed pull
//! request cancels its open run on every workflow of the repo, enabled or not.
//!
//! A read that fails records a refused run at the payload's head on each workflow that would
//! fire, with the reason, and nothing retries it. A close whose read fails cancels nothing and
//! notes `close_unconfirmed`, with the reason, in the history of each open run of that pull
//! request; no open run, no note.

use moor_core::name::ForgeRepo;
use moor_core::workflow::{
    CancelCause, CommitSha, HistoryEntry, NewRun, PullRef, PullTarget, RunCause, RunEnd, RunState,
    Workflow, WorkflowState,
};

use super::cancel::cancel_pull;
use super::eligible::{read_back, Verdict};
use super::now::{fire, Start};
use crate::cx::Cx;
use crate::store::Store;

/// What a payload asks for, before GitHub is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PullAction {
    /// Opened, reopened, marked ready for review or pushed to.
    Fire,
    /// Closed, merged or not.
    Close,
}

/// A pull request event as its payload names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullEvent {
    /// The pull request, its repo as the payload spells it.
    pub(crate) target: PullTarget,
    /// The head the payload names. Only a read that fails records a run at it.
    pub(crate) head_sha: CommitSha,
    /// What the payload asks for.
    pub(crate) action: PullAction,
}

/// Handles `event` for every workflow on its repo, handing each recorded claim to `start`.
/// Returns what to log.
pub(crate) fn on_pull_request(
    cx: &Cx,
    store: &Store,
    event: &PullEvent,
    start: Start<'_>,
) -> Vec<String> {
    let target = &event.target;
    let workflows = match store.workflows_on(&target.repo) {
        Ok(found) => found,
        Err(e) => return vec![format!("pull request {target}: {e}")],
    };
    if workflows.is_empty() {
        return Vec::new();
    }
    let verdict = match read_back(cx, target) {
        Ok(verdict) => verdict,
        Err(why) => return unread(cx, store, event, &workflows, &why),
    };
    let mut out = Vec::new();
    if let Verdict::Skip(why) = verdict {
        out.push(format!("pull request {target}: nothing fired, {why}"));
    }
    for w in &workflows {
        let Some(first) = w.repos.first() else {
            continue;
        };
        let at = |head_sha: &CommitSha| PullRef {
            repo: first.clone(),
            number: target.number,
            head_sha: head_sha.clone(),
        };
        match &verdict {
            Verdict::Closed => out.extend(closed(cx, store, w, (first, target))),
            Verdict::Fire { head_sha } if fires(w) => {
                let cause = RunCause::PullRequest { pull: at(head_sha) };
                let line = match fire(cx, store, w, &cause, start) {
                    Ok(v) => match v.reason {
                        Some(why) => format!("{}: {why}", v.status),
                        None => v.status,
                    },
                    Err(e) => format!("failed: {}", e.error),
                };
                out.push(said(w, target, head_sha, &line));
            }
            Verdict::Fork { head_sha, author } if fires(w) => {
                let state = RunState::Done(RunEnd::SkippedFork {
                    author: author.clone(),
                });
                let line = record(cx, store, w, (at(head_sha), state)).map_or_else(
                    |why| why,
                    |_| format!("skipped_fork: {author} cannot write to the repo"),
                );
                out.push(said(w, target, head_sha, &line));
            }
            Verdict::Fire { .. } | Verdict::Fork { .. } | Verdict::Skip(_) => {}
        }
    }
    out.extend(cx.take_warnings().into_iter().map(|w| w.message));
    out
}

/// Whether `w` fires on a pull request event: it is enabled and has the trigger on.
const fn fires(w: &Workflow) -> bool {
    w.on_pull_request && matches!(w.state, WorkflowState::Enabled { .. })
}

/// The log line for workflow `w` on `target` at `head_sha`.
fn said(w: &Workflow, target: &PullTarget, head_sha: &CommitSha, line: &str) -> String {
    format!(
        "workflow {} on {target} at {}: {line}",
        w.name,
        head_sha.short()
    )
}

/// Records a pull request run of `w` for `pull` that never starts a Box, already in `state`.
/// Whether it was recorded; a run at that head already holds the key otherwise. The error is
/// what to log.
fn record(
    cx: &Cx,
    store: &Store,
    w: &Workflow,
    (pull, state): (PullRef, RunState),
) -> Result<bool, String> {
    let now = cx.clock().now();
    let run = NewRun {
        workflow: w.id,
        repo: pull.repo.clone(),
        due_at: now,
        state,
        recorded_at: now,
        plan: None,
        cause: RunCause::PullRequest { pull },
    };
    store
        .claim_workflow_run(&run)
        .map(|id| id.is_some())
        .map_err(|e| format!("failed: {e}"))
}

/// What a read of `event`'s pull request that failed with `why` comes to. Each workflow that
/// would fire records a refused run at the payload's head, so the failure shows in its runs. A
/// close cancels nothing on a payload alone.
fn unread(
    cx: &Cx,
    store: &Store,
    event: &PullEvent,
    workflows: &[Workflow],
    why: &str,
) -> Vec<String> {
    let target = &event.target;
    let detail = format!("could not read {target} from GitHub: {why}");
    if event.action == PullAction::Close {
        return unconfirmed(cx, store, event, workflows, &detail);
    }
    let mut out = Vec::new();
    for w in workflows.iter().filter(|w| fires(w)) {
        let Some(first) = w.repos.first() else {
            continue;
        };
        let pull = PullRef {
            repo: first.clone(),
            number: target.number,
            head_sha: event.head_sha.clone(),
        };
        let state = RunState::Done(RunEnd::Refused {
            detail: detail.clone(),
        });
        let line = match record(cx, store, w, (pull, state)) {
            Ok(true) => format!("refused: {detail}"),
            Ok(false) => format!("{detail}; a run at this head is already recorded"),
            Err(e) => e,
        };
        out.push(said(w, target, &event.head_sha, &line));
    }
    out
}

/// Notes on the open run of each workflow of the repo that the close of `event`'s pull request
/// could not be confirmed, with `detail`. Nothing is cancelled and nothing retried; a workflow
/// with no open run for it records nothing. What to log.
fn unconfirmed(
    cx: &Cx,
    store: &Store,
    event: &PullEvent,
    workflows: &[Workflow],
    detail: &str,
) -> Vec<String> {
    let target = &event.target;
    let entry = HistoryEntry::CloseUnconfirmed {
        detail: detail.to_owned(),
    };
    let mut out = vec![format!(
        "pull request {target}: {detail}; nothing cancelled"
    )];
    for w in workflows {
        let Some(first) = w.repos.first() else {
            continue;
        };
        match store.note_pull_run(w.id, (first, target.number), (&entry, cx.clock().now())) {
            Ok(Some(run)) => out.push(format!(
                "workflow {} on {target}: run {} stays open, close not confirmed",
                w.name,
                run.get()
            )),
            Ok(None) => {}
            Err(e) => out.push(format!("workflow {} on {target}: failed: {e}", w.name)),
        }
    }
    out
}

/// Cancels the open run of `w` for `target`, which is on `first`, because it was closed. What
/// to log.
fn closed(
    cx: &Cx,
    store: &Store,
    w: &Workflow,
    (first, target): (&ForgeRepo, &PullTarget),
) -> Vec<String> {
    match cancel_pull(cx, store, w, (first, target.number), &CancelCause::Closed) {
        Ok((Some(_), mut out)) => {
            out.insert(
                0,
                format!("workflow {} on {target}: cancelled, closed", w.name),
            );
            out
        }
        Ok((None, out)) => out,
        Err(e) => vec![format!("workflow {} on {target}: {}", w.name, e.error)],
    }
}
