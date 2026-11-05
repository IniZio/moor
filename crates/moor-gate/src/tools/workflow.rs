//! The workflow tools: a signed-in user keeps recurring jobs that start a Box in each repo on
//! a cron, and reads what each run did.

use moor_core::request::{
    AsReq, Request as ControllerRequest, WorkflowAddReq, WorkflowEnableReq, WorkflowRemoveReq,
    WorkflowRunsReq,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;

use super::reply::Reply;
use super::Tools;
use crate::mcp::{READ, WRITE};

/// Arguments of `add_workflow`.
#[derive(Deserialize, JsonSchema)]
struct AddArgs {
    /// A name, unique among your workflows; adding the same name again replaces it.
    name: String,
    /// A five-field cron expression: minute, hour, day of month, month, day of week.
    cron: String,
    /// The IANA time zone the cron is read in, such as `Europe/London`.
    timezone: String,
    /// The repos, each as `owner/name`, that each firing starts a Box in.
    repos: Vec<String>,
    /// What each Box's agent is asked to do.
    prompt: String,
    /// Minutes a run may take before it is stopped, 1 to 720; 180 when left out.
    timeout_minutes: Option<u32>,
}

/// Arguments of the tools that name one workflow.
#[derive(Deserialize, JsonSchema)]
struct NameArgs {
    /// The workflow's name, as `list_workflows` shows it.
    name: String,
}

/// Arguments of `set_workflow_enabled`.
#[derive(Deserialize, JsonSchema)]
struct EnableArgs {
    /// The workflow's name, as `list_workflows` shows it.
    name: String,
    /// True to fire from now on, false to stop firing.
    enabled: bool,
}

/// Arguments of `list_workflow_runs`.
#[derive(Deserialize, JsonSchema)]
struct RunsArgs {
    /// The `next` of the previous call, to get only what changed since; everything when left out.
    after: Option<i64>,
    /// Most changes to return, up to 200; 50 when left out.
    limit: Option<u32>,
}

#[tool_router(router = workflow_router, vis = "pub(super)")]
impl Tools {
    /// Adds or replaces a workflow.
    #[tool(
        description = "Add a workflow: on its cron, moor starts a Box in each of the repos with \
                       your prompt. Every run acts as you, with your credentials, and its agent \
                       may push branches and open pull requests in those repos; it cannot merge \
                       them. A run is skipped while the previous run for that repo is unfinished, \
                       and a run past timeout_minutes is stopped. Three failed runs in a row for \
                       a repo disable the workflow. A cron that can fire closer together than the \
                       controller's minimum gap, an hour unless configured, is refused. Adding \
                       an existing name replaces it. Needs the moor:write scope."
    )]
    async fn add_workflow(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<AddArgs>,
    ) -> CallToolResult {
        let request = ControllerRequest::WorkflowAdd(WorkflowAddReq {
            identity: None,
            name: args.name,
            cron: args.cron,
            timezone: args.timezone,
            repos: args.repos,
            prompt: args.prompt,
            timeout_minutes: args.timeout_minutes,
            origin_key: None,
        });
        self.run(&context, WRITE, Ok(request), Reply::AsIs).await
    }

    /// Lists the caller's workflows.
    #[tool(
        description = "List your workflows: cron, time zone, repos, prompt, timeout, whether each \
                       is enabled, why it was disabled if the controller disabled it, its next \
                       due time and the latest run of each repo with its outcome and Box. Needs \
                       the moor:read scope."
    )]
    async fn list_workflows(&self, context: RequestContext<RoleServer>) -> CallToolResult {
        let request = ControllerRequest::WorkflowList(AsReq { identity: None });
        self.run(&context, READ, Ok(request), Reply::AsIs).await
    }

    /// Removes a workflow.
    #[tool(
        description = "Remove one of your workflows and its run history. Boxes it already \
                       started stay open. Needs the moor:write scope."
    )]
    async fn remove_workflow(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<NameArgs>,
    ) -> CallToolResult {
        let request = ControllerRequest::WorkflowRemove(WorkflowRemoveReq {
            identity: None,
            name: args.name,
        });
        self.run(&context, WRITE, Ok(request), Reply::AsIs).await
    }

    /// Enables or disables a workflow.
    #[tool(
        description = "Enable or disable one of your workflows. A workflow fires only for due \
                       times after it was last enabled, so enabling never runs missed times. \
                       Enabling one the controller disabled clears the reason. Needs the \
                       moor:write scope."
    )]
    async fn set_workflow_enabled(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<EnableArgs>,
    ) -> CallToolResult {
        let request = ControllerRequest::WorkflowSetEnabled(WorkflowEnableReq {
            identity: None,
            name: args.name,
            enabled: args.enabled,
        });
        self.run(&context, WRITE, Ok(request), Reply::AsIs).await
    }

    /// Reads changes to the caller's workflow runs.
    #[tool(
        description = "Read what your workflow runs did, oldest change first: each entry is a \
                       run (repo, due time, outcome, refusal detail, Box) or a workflow the \
                       controller disabled (with the reason). A run appears again each time it \
                       moves on, from started to finished or timed out. Pass the previous \
                       `next` as `after` to get only newer changes. Needs the moor:read scope."
    )]
    async fn list_workflow_runs(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<RunsArgs>,
    ) -> CallToolResult {
        let request = ControllerRequest::WorkflowRuns(WorkflowRunsReq {
            identity: None,
            after: args.after,
            limit: args.limit,
        });
        self.run(&context, READ, Ok(request), Reply::AsIs).await
    }
}
