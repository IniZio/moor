//! A pull request event from the App's webhook, for every workflow whose first repo it is on.
//!
//! Opened, reopened, ready for review and a push fire each enabled workflow that has
//! `on_pull_request`, through the claim a manual run for a pull request uses. A newer head
//! cancels the open run of an older one, and the same head again records nothing. A close
//! cancels the pull request's open run on every workflow of the repo, enabled or not.

use moor_core::name::ForgeRepo;
use moor_core::workflow::{
    CancelCause, CommitSha, PullRef, PullTarget, RunCause, Workflow, WorkflowState,
};

use super::cancel::cancel_pull;
use super::now::{fire, Start};
use crate::cx::Cx;
use crate::store::Store;

/// What a pull request event asks of the workflows on its repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PullEvent {
    /// Opened, reopened, marked ready for review or pushed to: fire at `head_sha`.
    Fire {
        /// The pull request, its repo as the payload spells it.
        target: PullTarget,
        /// Its head commit.
        head_sha: CommitSha,
    },
    /// Closed, merged or not: cancel its open runs.
    Closed(PullTarget),
}

impl PullEvent {
    /// The pull request the event is about.
    const fn target(&self) -> &PullTarget {
        match self {
            Self::Fire { target, .. } | Self::Closed(target) => target,
        }
    }
}

/// Handles `event` for every workflow on its repo, handing each recorded claim to `start`.
/// Returns what to log.
pub(crate) fn on_pull_request(
    cx: &Cx,
    store: &Store,
    event: &PullEvent,
    start: Start<'_>,
) -> Vec<String> {
    let target = event.target();
    let workflows = match store.workflows_on(&target.repo) {
        Ok(found) => found,
        Err(e) => return vec![format!("pull request {target}: {e}")],
    };
    let mut out = Vec::new();
    for w in &workflows {
        let Some(first) = w.repos.first() else {
            continue;
        };
        match event {
            PullEvent::Fire { head_sha, .. } if fires(w) => {
                let pull = PullRef {
                    repo: first.clone(),
                    number: target.number,
                    head_sha: head_sha.clone(),
                };
                let line = match fire(cx, store, w, &RunCause::PullRequest { pull }, start) {
                    Ok(v) => match v.reason {
                        Some(why) => format!("{}: {why}", v.status),
                        None => v.status,
                    },
                    Err(e) => format!("failed: {}", e.error),
                };
                out.push(format!(
                    "workflow {} on {target} at {}: {line}",
                    w.name,
                    head_sha.short()
                ));
            }
            PullEvent::Fire { .. } => {}
            PullEvent::Closed(_) => out.extend(closed(cx, store, w, (first, target))),
        }
    }
    out.extend(cx.take_warnings().into_iter().map(|w| w.message));
    out
}

/// Whether `w` fires on a pull request event: it is enabled and has the trigger on.
const fn fires(w: &Workflow) -> bool {
    w.on_pull_request && matches!(w.state, WorkflowState::Enabled { .. })
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
