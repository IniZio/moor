//! The workflow tools: a signed-in user keeps recurring jobs that start one Box over a set of
//! repos on a cron, and reads what each run did.

use moor_core::config::override_text;
use moor_core::request::{
    AsReq, Request as ControllerRequest, WorkflowAddReq, WorkflowAnswerReq, WorkflowEnableReq,
    WorkflowRemoveReq, WorkflowRunNowReq, WorkflowRunsReq,
};
use moor_core::workflow::{Answer, PlanDoc, PullTarget};
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
    /// The repos each run's one Box clones, each as `owner/name`, first one first. The agent
    /// works in the first, where its branch and pull request go; the rest are cloned beside it,
    /// and each must be one you can push to.
    repos: Vec<String>,
    /// A partial repo config, as YAML in the repo config format, laid over the repo's stored
    /// config for every Box of the workflow: a key you set wins, `env` merges by name, lists
    /// replace. `proxy.allow` adds extra `METHOD /path` entries, each with `{repo}`, to a default
    /// Proxy rule for these Boxes only. None when left out.
    config: Option<String>,
    /// What each Box's agent is asked to do.
    prompt: String,
    /// Minutes a run may take before it is stopped, 1 to 720; 180 when left out.
    timeout_minutes: Option<u32>,
    /// The plan a run walks, as an object with a `steps` list; see the tool description for how
    /// to write one. None when left out, and each run then delivers the prompt in one step.
    #[schemars(schema_with = "plan_schema")]
    plan: Option<serde_json::Value>,
    /// The `revision` `list_workflows` showed for the workflow you are replacing. The call is
    /// refused when the workflow has changed since. Leave it out for a new workflow.
    expected_revision: Option<u64>,
    /// False to store the workflow disabled in this same write, so no cron fires before you
    /// call `set_workflow_enabled`; true when left out.
    enabled: Option<bool>,
}

/// The JSON Schema of a plan, which `add_workflow` checks the argument against.
fn plan_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    generator.subschema_for::<Option<PlanDoc>>()
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

/// What the owner answers.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum AnswerArg {
    /// Go on to the step after the approve step.
    Yes,
    /// Take the approve step's fail edge, or end the run declined when it has none.
    No,
}

/// Arguments of `answer_run`.
#[derive(Deserialize, JsonSchema)]
struct AnswerArgs {
    /// The workflow's name, as `list_workflows` shows it.
    workflow: String,
    /// The pull request whose run to answer, `OWNER/NAME#NUMBER`, as `list_workflow_runs`
    /// shows it. Needed when several runs of the workflow wait; left out, it answers the run for
    /// no pull request, or the only one that waits.
    pull: Option<String>,
    /// `yes` or `no`.
    answer: AnswerArg,
    /// What to tell the agent along with the answer; none when left out.
    note: Option<String>,
}

/// Arguments of `run_workflow`.
#[derive(Deserialize, JsonSchema)]
struct RunNowArgs {
    /// The workflow's name, as `list_workflows` shows it.
    name: String,
    /// The `revision` `list_workflows` showed for the workflow. The call is refused when the
    /// workflow has changed since, so the run tries the plan you reviewed.
    expected_revision: Option<u64>,
    /// A pull request on the workflow's first repo, `OWNER/NAME#NUMBER`, to run for at its
    /// current head. Needs a GitHub App in the controller config.
    pull: Option<String>,
}

/// Arguments of `list_workflow_runs`.
#[derive(Deserialize, JsonSchema)]
struct RunsArgs {
    /// The `next` of the previous call, to get only what changed since; everything when left out.
    after: Option<i64>,
    /// Most changes to return, up to 200; 50 when left out.
    limit: Option<u32>,
    /// A workflow name, as `list_workflows` shows it, to read only that workflow's changes;
    /// every workflow's when left out.
    workflow: Option<String>,
}

#[tool_router(router = workflow_router, vis = "pub(super)")]
impl Tools {
    /// Adds or replaces a workflow.
    #[tool(
        description = "Add a workflow: on its cron, moor starts one Box with your prompt that \
                       clones every repo in `repos`. The agent starts in the first repo, and the \
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
                       and a run past timeout_minutes is stopped. Three failed runs in a row \
                       disable the workflow. Pass `enabled: false` to store it disabled in the same \
                       write, so no cron fires before you enable it with set_workflow_enabled. \
                       A cron that can fire closer together than the \
                       controller's minimum gap, an hour unless configured, is refused. Adding \
                       an existing name replaces it, plan included: send the whole plan again, \
                       and pass the revision you read as expected_revision, so a change made \
                       since is not overwritten. \
                       \
                       Write a plan for any job with more than one finish line; a prompt with no \
                       plan runs as one step. Turn the prompt into `plan.steps`, a short list \
                       the author reviews: one step per finish line. Use `work` steps for what \
                       the agent does, each with the evidence files it leaves in \
                       .moor/artifacts/ (flat names such as `report.md`, or a prefix such as \
                       `before-*`), and a final `deliver` step for the pull request, message or \
                       file the prompt asks for. A `deliver` step has no `ok` or `fail` and ends \
                       the run; every other step needs an `ok` edge to a step id, and every step \
                       must lead on to a deliver step. A `fail` edge goes to the step to run \
                       when a step cannot be done; without one the run fails. Give a work step \
                       that loops back `max_tries`, 1 to 5. A `check` step is run by moor in the \
                       Box, not by the agent: exit 0 follows `ok`, any other exit follows \
                       `fail`, and the command may run twice if a connection drops, so it must \
                       be safe to repeat. Use a check step for anything a command can decide. \
                       An `approve` step asks the owner a `question` and the run waits, with \
                       its Box left to sleep, until the owner calls answer_run; yes follows \
                       `ok`, no follows `fail`, and a no with no `fail` edge ends the run \
                       declined. A wait with no answer ends the run declined after 7 days. \
                       When you change an \
                       existing workflow, read its plan with list_workflows and keep the ids of \
                       the steps that did not change, so run history stays attached to them. \
                       Leave `made_from` out of a plan you wrote for the prompt you send; \
                       list_workflows reports `plan_stale` when the prompt changed after the \
                       plan was made. Read get_repo_config first, and say in your reply when a \
                       step needs a host or credential the repo does not grant; moor does not \
                       judge that. Nothing is saved until the plan is valid: every error is \
                       listed at once, so fix them all and call again. The reply lists the \
                       stored plan as numbered steps and the new revision. Show the numbered \
                       steps to the author next to the prompt, and change them on request with \
                       another call. Needs the moor:write scope."
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
            config: override_text(args.config),
            prompt: args.prompt,
            timeout_minutes: args.timeout_minutes,
            // A model may send the plan as an object or as the JSON text of one.
            plan: args.plan.map(|p| match p {
                serde_json::Value::String(text) => text,
                other => other.to_string(),
            }),
            expected_revision: args.expected_revision,
            origin_key: None,
            keep_state: false,
            enabled: args.enabled.unwrap_or(true),
        });
        self.run(&context, WRITE, Ok(request), Reply::AsIs).await
    }

    /// Lists the caller's workflows.
    #[tool(
        description = "List your workflows: cron, time zone, repos, config (which may hold the extra Proxy \
                       `allow` entries the workflow's Boxes get, and only theirs), prompt, plan (with \
                       plan_stale when the prompt changed after it was made), revision, timeout, whether each \
                       is enabled, why it was disabled if the controller disabled it, its next \
                       due time and its latest run with its outcome and Box. Needs \
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
        let pull = args
            .pull
            .as_deref()
            .map(PullTarget::parse)
            .transpose()
            .map_err(|e| e.to_string());
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
                       manual run that fails does not count toward the three failed runs that \
                       disable a workflow, and one that delivers resets that count. Pass the \
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
        let pull = args
            .pull
            .as_deref()
            .map(PullTarget::parse)
            .transpose()
            .map_err(|e| e.to_string());
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
    #[tool(
        description = "Read what your workflow runs did, oldest change first: each entry is a \
                       run (due time, outcome, refusal detail, Box) or a workflow the \
                       controller disabled (with the reason). A run appears again each time it \
                       moves on, from started to finished or timed out. Pass the previous \
                       `next` as `after` to get only newer changes, and `workflow` to read one \
                       workflow's changes only. Needs the moor:read scope."
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
            workflow: args.workflow,
        });
        self.run(&context, READ, Ok(request), Reply::AsIs).await
    }
}
