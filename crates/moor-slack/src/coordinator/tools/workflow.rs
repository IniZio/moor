//! The workflow tools: a person's recurring jobs on the controller, which start a Box in each
//! repo on a cron. They send nothing without the thread's starter having written in this turn,
//! and ask the controller as the starter, whose credentials every run uses.

use moor_core::request::{
    AsReq, Request, WorkflowAddReq, WorkflowEnableReq, WorkflowRemoveReq, WorkflowRunsReq,
};
use moor_core::response::ExitClass;

use super::{error_text, Call, Reply, Scope};

/// The key the controller hands back with every run of a workflow added in this thread.
pub(crate) fn origin_key(channel: &str, thread_ts: &str) -> String {
    format!("{channel}/{thread_ts}")
}

/// Runs a workflow `call`: the starter gate, then one request.
pub(super) fn run(scope: &mut Scope<'_>, call: &Call) -> rusqlite::Result<Reply> {
    let db = scope.db;
    let Some(turn) = db.open_turn(scope.channel, scope.thread_ts)? else {
        return Ok(Reply::refused("No turn of this thread is running."));
    };
    let Some(thread) = db.thread(scope.channel, scope.thread_ts)? else {
        return Ok(Reply::refused("The front keeps no record of this thread."));
    };
    let by = thread.owner;
    if !turn.authors.contains(&by) {
        return Ok(Reply::refused(format!(
            "Only <@{by}> can use workflows in this thread, and <@{by}> wrote nothing in this turn."
        )));
    }
    let request = match call {
        Call::AddWorkflow {
            name,
            cron,
            timezone,
            repos,
            prompt,
            timeout_minutes,
        } => Request::WorkflowAdd(WorkflowAddReq {
            identity: None,
            name: name.clone(),
            cron: cron.clone(),
            timezone: timezone.clone(),
            repos: repos.clone(),
            prompt: prompt.clone(),
            timeout_minutes: *timeout_minutes,
            origin_key: Some(origin_key(scope.channel, scope.thread_ts)),
        }),
        Call::ListWorkflows => Request::WorkflowList(AsReq { identity: None }),
        Call::RemoveWorkflow { name } => Request::WorkflowRemove(WorkflowRemoveReq {
            identity: None,
            name: name.clone(),
        }),
        Call::SetWorkflowEnabled { name, enabled } => {
            Request::WorkflowSetEnabled(WorkflowEnableReq {
                identity: None,
                name: name.clone(),
                enabled: *enabled,
            })
        }
        Call::ListWorkflowRuns { after, limit } => Request::WorkflowRuns(WorkflowRunsReq {
            identity: None,
            after: *after,
            limit: *limit,
        }),
        _ => return Ok(Reply::refused("Not a workflow tool.")),
    };
    Ok(match (scope.call)(request) {
        Ok(r) if r.class == ExitClass::Ok => {
            if matches!(call, Call::AddWorkflow { .. }) {
                db.follow_workflows(&by)?;
            }
            Reply::done(r.value.map_or_else(|| "done".to_owned(), |v| v.to_string()))
        }
        Ok(r) => Reply::refused(format!("The controller refused: {}", error_text(r))),
        Err(e) => Reply::refused(format!("The controller did not answer: {e}")),
    })
}
