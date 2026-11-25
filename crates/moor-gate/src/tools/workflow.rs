//! The workflow tools: a signed-in user keeps recurring jobs that start one Box over a set of
//! repos on a cron, and reads what each run did.

use moor_core::request::{
    AsReq, Request as ControllerRequest, WorkflowAddReq, WorkflowAnswerReq, WorkflowEnableReq,
    WorkflowRemoveReq, WorkflowRunListReq, WorkflowRunNowReq, WorkflowRunsReq,
};
use moor_core::workflow::Answer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};

use super::reply::Reply;
use super::Tools;
use crate::mcp::{READ, WRITE};
use args::{
    pull_arg, AddArgs, AnswerArg, AnswerArgs, EnableArgs, NameArgs, RunArgs, RunNowArgs, RunsArgs,
};

#[tool_router(router = workflow_router, vis = "pub(super)")]
impl Tools {
    /// Adds or replaces a workflow.
    #[tool(
        description = "Add a workflow: on its cron, moor starts one Box with your prompt that \
                       clones every repo in `repos`. The cron is optional: leave it out, with \
                       `on_pull_request` or alone, for a workflow that runs only on pull requests \
                       or by hand with run_workflow. The agent starts in the first repo, and the \
                       rest sit beside it. A workflow that needs a GitHub write the default Proxy \
                       rules refuse, such as POST or PATCH of an issue comment, or POST of a commit status, \
                       asks for it in `config`, under `proxy.allow` on the default rule `github-api`, \
                       for example `proxy: {allow: {github-api: [\"PATCH /repos/{repo}/issues/comments/*\"]}}`. \
                       The entries apply to this workflow's Boxes only. Each must be `METHOD /path` \
                       with `{repo}`, never `{any_repo}` or `*`; `{repo}` expands to every repo of the Box, \
                       the first and the others, one entry each; no new hosts or credentials. \
                       moor refuses anything else with the reason. To run the same job on separate projects, add one \
                       workflow per project. Every run acts as you, with your credentials, and its agent \
                       may push branches and open pull requests in those repos; it cannot merge \
                       them. A run is skipped while the workflow's previous run is unfinished, \
                       and a run past timeout_minutes is stopped. A failed run never disables the \
                       workflow: its next cron still fires. Pass `enabled: false` to store it disabled in the same \
                       write, so no cron fires before you enable it with set_workflow_enabled. \
                       A cron that can fire closer together than the \
                       controller's minimum gap, an hour unless configured, is refused. Adding \
                       an existing name replaces it: pass the revision you read as expected_revision, so a \
                       change made since is not overwritten. Every other field you leave out keeps its stored value: \
                       `cron` and `timezone`, `config`, `timeout_minutes`, `enabled` and `on_pull_request`. \
                       To clear the cron send `cron: null`; to clear `config` send an empty string. A new \
                       workflow uses the defaults instead. \
                       \
                       Write the prompt only. moor reads it and keeps its own reading: a plan of the \
                       steps, or gates and outcomes when the prompt has runs pass named gates, made \
                       again whenever the prompt changes. Gates a person wrote (`gates_by: person`) are \
                       kept through a prompt change. Never send a plan, gates or outcomes: a call that \
                       names one is refused. get_workflow shows the plan or gates as moor's current \
                       reading of the prompt, and `plan_making` while moor makes it or why it made \
                       none. Send `regenerate_plan: true` with the same prompt to have moor make it \
                       again; once made, it replaces the stored plan or gates, those a person wrote \
                       too, which stay in force until then. \
                       Needs the moor:write scope."
    )]
    async fn add_workflow(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<AddArgs>,
    ) -> CallToolResult {
        let request = ControllerRequest::WorkflowAdd(WorkflowAddReq {
            identity: None,
            name: args.name,
            // Null clears the cron, which the controller reads as a blank one.
            cron: args.cron.map(Option::unwrap_or_default),
            timezone: args.timezone,
            repos: args.repos,
            config: args.config,
            prompt: args.prompt,
            timeout_minutes: args.timeout_minutes,
            expected_revision: args.expected_revision,
            origin_key: None,
            keep_state: false,
            enabled: args.enabled,
            on_pull_request: args.on_pull_request,
            regenerate_plan: args.regenerate_plan.unwrap_or(false),
        });
        self.run(&context, WRITE, Ok(request), Reply::AsIs).await
    }

    /// Lists the caller's workflows.
    #[tool(
        description = "List your workflows, one compact row each: name, enabled, cron and time \
                       zone (none for a workflow with no cron), repos, next_due, last_run (one \
                       line: when, outcome, Box), the names of its gates and a title when its prompt \
                       opens with a heading. The rows leave out the prompt, plan, config, gates, \
                       outcomes, revision and run detail: call get_workflow with a workflow's \
                       name for those. Needs the moor:read scope."
    )]
    async fn list_workflows(&self, context: RequestContext<RoleServer>) -> CallToolResult {
        let request = ControllerRequest::WorkflowList(AsReq { identity: None });
        self.run(&context, READ, Ok(request), Reply::WorkflowsListed)
            .await
    }

    /// Reads one workflow in full.
    #[tool(
        description = "Read one of your workflows in full: cron and time zone, repos, config \
                       (which may hold the extra Proxy `allow` entries the workflow's Boxes get, \
                       and only theirs), prompt, plan (with plan_stale when the prompt changed \
                       after the plan or gates were made), revision, timeout, whether it is enabled, \
                       its gates and outcomes (with gates_by: person when a person wrote them), its next due time and its latest run with its outcome, Box and \
                       history. Read it before changing it with add_workflow. Needs the moor:read \
                       scope."
    )]
    async fn get_workflow(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<NameArgs>,
    ) -> CallToolResult {
        let request = ControllerRequest::WorkflowList(AsReq { identity: None });
        self.run(&context, READ, Ok(request), Reply::WorkflowOne(args.name))
            .await
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
                       Needs the moor:write scope."
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

    /// Answers the approve step a run waits at.
    #[tool(
        description = "Answer the approve step one of your workflow runs waits at: `yes` goes on \
                       along the step's ok edge, `no` along its fail edge, or ends the run \
                       declined when it has none. The note is passed to the agent with the \
                       next step, which wakes the run's Box. Find waiting runs with \
                       list_workflows (phase `waiting`). Each pull request has its own run, so \
                       when several runs of the workflow wait, pass `pull`, such as \
                       `owner/name#12`, to say which; without it the run for no pull request \
                       is answered, or the only one that waits, and otherwise the call is \
                       refused with the list of waiting runs. Only the workflow's owner can answer. \
                       Needs the moor:write scope."
    )]
    async fn answer_run(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<AnswerArgs>,
    ) -> CallToolResult {
        let pull = pull_arg(args.pull.as_deref());
        let request = pull.map(|pull| {
            ControllerRequest::WorkflowAnswer(WorkflowAnswerReq {
                identity: None,
                workflow: args.workflow,
                pull,
                answer: match args.answer {
                    AnswerArg::Yes => Answer::Yes,
                    AnswerArg::No => Answer::No,
                },
                note: args.note,
            })
        });
        self.run(&context, WRITE, request, Reply::AsIs).await
    }

    /// Fires a workflow once, now.
    #[tool(
        description = "Run one of your workflows once, now, in one Box with all its repos, \
                       instead of waiting for its cron. It works on a disabled workflow, so you can \
                       add one, disable it, run it until the plan is right, and enable it \
                       after. The run is an ordinary run: it freezes the plan, starts a Box as \
                       you, walks the steps and has the workflow's timeout. It shows in \
                       list_workflow_runs with trigger `manual`. A workflow holds one open run \
                       per pull request and one for no pull request. A run whose subject has a \
                       run open (starting, working, waiting for an answer, or timed out with \
                       its Box still open) is refused with that run's Box, and nothing is \
                       recorded; a run for a newer head of a pull request instead cancels the \
                       open run of the older head and stops its agent. A cron run that comes due while a manual run for no pull request is open is skipped. A \
                       manual run that fails is listed as failed like any other. Pass the \
                       revision you read as expected_revision to be sure the run tries the plan \
                       you reviewed. Pass `pull`, such as `owner/name#12`, to run for that pull \
                       request of the workflow's first repo at its current head, which moor \
                       reads as the controller's GitHub App; without an App it is refused. The \
                       Box then gets MOOR_PULL_REPO, MOOR_PULL_NUMBER, MOOR_PULL_HEAD_SHA and \
                       MOOR_PULL_URL in its env, and its name carries the pull request. \
                       The reply says `started` with the Box name, or \
                       `refused` with the reason. It returns once the run is \
                       recorded and its Box start has begun, while setup continues; follow \
                       the Box with box_status and the run with list_workflow_runs, where a \
                       start refused later shows as a refused run. Needs the moor:write \
                       scope."
    )]
    async fn run_workflow(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<RunNowArgs>,
    ) -> CallToolResult {
        let pull = pull_arg(args.pull.as_deref());
        let request = pull.map(|pull| {
            ControllerRequest::WorkflowRunNow(WorkflowRunNowReq {
                identity: None,
                name: args.name,
                expected_revision: args.expected_revision,
                pull,
            })
        });
        self.run(&context, WRITE, request, Reply::AsIs).await
    }

    /// Reads changes to the caller's workflow runs.
    #[tool(description = "Read what your workflow runs did: each entry is a \
                       compact row for a run (id, workflow, due_at, outcome, a one-line detail \
                       when it was refused or failed, Box). A run appears again each time it \
                       moves on, from started to finished or timed out. The rows leave out the \
                       run's history and result: call get_workflow_run with the workflow and the \
                       run's id for those. For a run of a Box \
                       with gates, `verdict` names what it found, such as `check failed`, while \
                       `outcome` stays `finished`. Without `after` you get the latest `limit` changes, oldest first \
                       within that page; pass the previous `next` as `after` to get only newer \
                       changes, or `after` 0 to read from the oldest change, `workflow` to read one \
                       workflow's changes only, and `pull`, such as `owner/name#12`, to read \
                       only the runs for that pull request. The filters apply before `limit`, \
                       so paging with `after` still works. To follow runs as they move, use \
                       `moor listen --workflow NAME` rather than calling this in a loop. Needs \
                       the moor:read scope.")]
    async fn list_workflow_runs(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<RunsArgs>,
    ) -> CallToolResult {
        let pull = pull_arg(args.pull.as_deref());
        let request = pull.map(|pull| {
            ControllerRequest::WorkflowRuns(WorkflowRunsReq {
                identity: None,
                after: args.after,
                limit: args.limit,
                workflow: args.workflow,
                pull,
            })
        });
        self.run(&context, READ, request, Reply::RunsListed).await
    }

    /// Reads one workflow run in full.
    #[tool(
        description = "Read one run of one of your workflows in full: due time, outcome, verdict, \
                       trigger, pull request, step, result, the whole refusal or failure detail, \
                       Box, the plan frozen into it and its history, oldest line first. Pass the \
                       workflow's name and the `id` of a row of list_workflow_runs. Needs the \
                       moor:read scope."
    )]
    async fn get_workflow_run(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<RunArgs>,
    ) -> CallToolResult {
        let request = ControllerRequest::WorkflowRunList(WorkflowRunListReq {
            identity: None,
            workflow: args.workflow,
            run: Some(args.run),
            limit: None,
        });
        self.run(&context, READ, Ok(request), Reply::RunOne(args.run))
            .await
    }
}

mod args;
