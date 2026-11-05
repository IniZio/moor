//! Arguments of the `workflow-*` requests, each acting as the caller's user.

use serde::{Deserialize, Serialize};

use super::{AsReq, Request};
use crate::user::Identity;

/// Arguments of `workflow-add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAddReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The workflow's name, unique per user; adding a name again replaces it.
    pub name: String,
    /// Five-field cron expression.
    pub cron: String,
    /// IANA time zone the cron is read in.
    pub timezone: String,
    /// The repos, each `OWNER/NAME`, that each firing starts a Box in.
    pub repos: Vec<String>,
    /// What each Box's agent is asked to do.
    pub prompt: String,
    /// Minutes a run may take before it is stopped; the default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_minutes: Option<u32>,
    /// A key the front that sends this uses to find the workflow's runs again. Only a front
    /// socket may send one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_key: Option<String>,
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
}

/// A workflow request without its identity, as the controller runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Read changes to runs after a cursor.
    Runs {
        /// The cursor.
        after: Option<i64>,
        /// Most changes to return.
        limit: Option<u32>,
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
            Self::Runs { .. } => "workflow-runs",
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
        Request::WorkflowRuns(r) => (
            r.identity,
            WorkflowCmd::Runs {
                after: r.after,
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
        | Request::WorkflowRuns(WorkflowRunsReq { identity, .. }) => identity.as_ref(),
        _ => None,
    }
}
