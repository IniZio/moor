//! Arguments of the `workflow-*` requests, each acting as the caller's user.

use serde::{Deserialize, Serialize};

use super::{AsReq, Request};
use crate::gate::GateDoc;
use crate::user::Identity;
use crate::workflow::{Answer, PullTarget};

/// Arguments of `workflow-add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAddReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The workflow's name, unique per user; adding a name again replaces it.
    pub name: String,
    /// Five-field cron expression. Absent keeps a stored workflow's cron; blank clears it, for a
    /// workflow that runs only by hand or on a pull request. A new workflow has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// IANA time zone the cron is read in; required with a cron, ignored without one. Absent
    /// keeps a stored workflow's zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// The repos, each `OWNER/NAME`, that each firing's one Box clones, in order. Its agent works
    /// in the first and the rest sit beside it. The controller's own `box.also` never applies.
    pub repos: Vec<String>,
    /// A partial repo config, in the repo config format, laid over each repo's config for the
    /// Boxes this workflow starts. Absent keeps a stored workflow's config (a new one has none);
    /// blank clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// What each Box's agent is asked to do.
    pub prompt: String,
    /// Minutes a run may take before it is stopped. Absent keeps a stored workflow's timeout; a
    /// new one gets the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_minutes: Option<u32>,
    /// The plan a run walks, as JSON text; none stores no plan, so a run delivers the prompt in
    /// one step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// The revision of the stored workflow this replaces, as `list_workflows` shows it. The call
    /// is refused when the workflow has been changed since. Not checked when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    /// A key the front that sends this uses to find the workflow's runs again. Only a front
    /// socket may send one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_key: Option<String>,
    /// Yes to replace a stored workflow's content and keep whether it is enabled, and the
    /// reason the controller disabled it for. A workflow just made is enabled either way. No
    /// (the default) is a replace that follows `enabled`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_state: bool,
    /// No to store the workflow disabled in the same write, so no cron fires before a later
    /// enable; yes to enable it. Absent keeps a stored workflow's state, as `keep_state` does,
    /// and enables a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Yes to fire a run when a pull request on the first repo is opened, reopened, marked
    /// ready for review or pushed to. Absent keeps a stored workflow's value; a new one fires
    /// on the cron and by hand only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_pull_request: Option<bool>,
    /// The gates each run's Box starts with, in place of a plan: the run is delivered only when
    /// they all pass. Absent keeps a stored workflow's gates; an empty list clears them. Sent
    /// with a plan, or kept beside one, the call is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gates: Option<Vec<GateDoc>>,
    /// The outcomes a run may end in, sent with `gates` and kept with them; left out, a run
    /// ends in `done` once every gate passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[allow(
        clippy::box_collection,
        reason = "keeps `Request` and the CLI enums that hold it under the large-variant size"
    )]
    pub outcomes: Option<Box<Vec<crate::gate::OutcomeDoc>>>,
}

/// Arguments of `workflow-remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRemoveReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The workflow's name.
    pub name: String,
}

/// Arguments of `workflow-set-enabled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEnableReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The workflow's name.
    pub name: String,
    /// Whether it fires.
    pub enabled: bool,
}

/// Arguments of `workflow-answer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAnswerReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The workflow's name.
    pub workflow: String,
    /// The pull request whose run to answer; the run for no pull request, or the only waiting
    /// run, when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<PullTarget>,
    /// Yes or no.
    pub answer: Answer,
    /// What the owner adds, passed on to the agent; none when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Arguments of `workflow-run-now`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRunNowReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The workflow's name.
    pub name: String,
    /// The revision of the stored workflow the caller read. The call is refused when the
    /// workflow has been changed since. Not checked when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    /// The pull request to run for, on the workflow's first repo. The controller reads its head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<PullTarget>,
}

/// Arguments of `workflow-runs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRunsReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The `next` of the previous reply; from the start when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<i64>,
    /// Most changes to return; 50 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Only the changes of the workflow with this name; every workflow's when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
}

/// Arguments of `workflow-run-list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRunListReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The workflow's name.
    pub workflow: String,
    /// One run, by the id a list showed, with its frozen plan; the latest runs when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<i64>,
    /// Most runs to return; 30 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A workflow request without its identity, as the controller runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    clippy::large_enum_variant,
    reason = "one value per request, built and matched once; boxing the add would touch every caller"
)]
pub enum WorkflowCmd {
    /// Add or replace a workflow. `origin_front` is the front socket the request arrived on,
    /// which the controller sets and a client never sends.
    Add {
        /// The request.
        req: WorkflowAddReq,
        /// The namespace of the front socket it arrived on, if it arrived on one.
        origin_front: Option<String>,
    },
    /// List the workflows.
    List,
    /// Remove a workflow.
    Remove {
        /// Its name.
        name: String,
    },
    /// Enable or disable a workflow.
    SetEnabled {
        /// Its name.
        name: String,
        /// Whether it fires.
        enabled: bool,
    },
    /// Answer the approve step a run waits at.
    Answer {
        /// The workflow's name.
        workflow: String,
        /// The pull request whose run to answer.
        pull: Option<PullTarget>,
        /// Yes or no.
        answer: Answer,
        /// What the owner adds.
        note: Option<String>,
    },
    /// Fire a workflow once, now.
    RunNow {
        /// The workflow's name.
        name: String,
        /// The revision the caller read.
        expected_revision: Option<u64>,
        /// The pull request to run for.
        pull: Option<PullTarget>,
    },
    /// Read the latest runs of one workflow.
    RunList {
        /// The workflow's name.
        workflow: String,
        /// One run, by id.
        run: Option<i64>,
        /// Most runs to return.
        limit: Option<u32>,
    },
    /// Read changes to runs after a cursor.
    Runs {
        /// The cursor.
        after: Option<i64>,
        /// Most changes to return.
        limit: Option<u32>,
        /// Only this workflow's changes.
        workflow: Option<String>,
    },
}

impl WorkflowCmd {
    /// The command name of the request that carried it.
    #[must_use]
    pub const fn cmd(&self) -> &'static str {
        match self {
            Self::Add { .. } => "workflow-add",
            Self::List => "workflow-list",
            Self::Remove { .. } => "workflow-remove",
            Self::SetEnabled { .. } => "workflow-set-enabled",
            Self::Answer { .. } => "workflow-answer",
            Self::RunNow { .. } => "workflow-run-now",
            Self::Runs { .. } => "workflow-runs",
            Self::RunList { .. } => "workflow-run-list",
        }
    }
}

/// Splits a workflow request into the identity it asserts and its command; any other request
/// comes back unchanged.
pub(super) fn split(request: Request) -> Result<(Option<Identity>, WorkflowCmd), Box<Request>> {
    Ok(match request {
        Request::WorkflowAdd(r) => (
            r.identity.clone(),
            WorkflowCmd::Add {
                req: r,
                origin_front: None,
            },
        ),
        Request::WorkflowList(AsReq { identity }) => (identity, WorkflowCmd::List),
        Request::WorkflowRemove(r) => (r.identity, WorkflowCmd::Remove { name: r.name }),
        Request::WorkflowSetEnabled(r) => (
            r.identity,
            WorkflowCmd::SetEnabled {
                name: r.name,
                enabled: r.enabled,
            },
        ),
        Request::WorkflowAnswer(r) => (
            r.identity,
            WorkflowCmd::Answer {
                workflow: r.workflow,
                pull: r.pull,
                answer: r.answer,
                note: r.note,
            },
        ),
        Request::WorkflowRunNow(r) => (
            r.identity,
            WorkflowCmd::RunNow {
                name: r.name,
                expected_revision: r.expected_revision,
                pull: r.pull,
            },
        ),
        Request::WorkflowRuns(r) => (
            r.identity,
            WorkflowCmd::Runs {
                after: r.after,
                limit: r.limit,
                workflow: r.workflow,
            },
        ),
        Request::WorkflowRunList(r) => (
            r.identity,
            WorkflowCmd::RunList {
                workflow: r.workflow,
                run: r.run,
                limit: r.limit,
            },
        ),
        other => return Err(Box::new(other)),
    })
}

/// The identity a workflow request's own field asserts.
pub(super) const fn own_identity(request: &Request) -> Option<&Identity> {
    match request {
        Request::WorkflowAdd(WorkflowAddReq { identity, .. })
        | Request::WorkflowList(AsReq { identity })
        | Request::WorkflowRemove(WorkflowRemoveReq { identity, .. })
        | Request::WorkflowSetEnabled(WorkflowEnableReq { identity, .. })
        | Request::WorkflowAnswer(WorkflowAnswerReq { identity, .. })
        | Request::WorkflowRunNow(WorkflowRunNowReq { identity, .. })
        | Request::WorkflowRunList(WorkflowRunListReq { identity, .. })
        | Request::WorkflowRuns(WorkflowRunsReq { identity, .. }) => identity.as_ref(),
        _ => None,
    }
}
