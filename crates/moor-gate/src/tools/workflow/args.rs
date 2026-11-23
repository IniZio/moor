//! The arguments of the workflow tools, and the pull request a tool argument names.

use moor_core::gate::GateDoc;
use moor_core::workflow::{PlanDoc, PullTarget};
use schemars::JsonSchema;
use serde::Deserialize;

/// Arguments of `add_workflow`.
#[derive(Deserialize, JsonSchema)]
pub(super) struct AddArgs {
    /// A name, unique among your workflows; adding the same name again replaces it.
    pub(super) name: String,
    /// A five-field cron expression: minute, hour, day of month, month, day of week. A new
    /// workflow with none runs only by hand (`run_workflow`) or on pull requests. When you
    /// replace a workflow, leave it out to keep its cron, or send null to clear it.
    #[serde(default, deserialize_with = "some")]
    #[allow(
        clippy::option_option,
        reason = "left out keeps the cron and null clears it, which Option<String> cannot tell apart"
    )]
    pub(super) cron: Option<Option<String>>,
    /// The IANA time zone the cron is read in, such as `Europe/London`. Required with a cron;
    /// when you replace a workflow, left out keeps its zone.
    pub(super) timezone: Option<String>,
    /// The repos each run's one Box clones, each as `owner/name`, first one first. The agent
    /// works in the first, where its branch and pull request go; the rest are cloned beside it,
    /// and each must be one you can push to.
    pub(super) repos: Vec<String>,
    /// A partial repo config, as YAML in the repo config format, laid over the repo's stored
    /// config for every Box of the workflow: a key you set wins, `env` merges by name, lists
    /// replace. `proxy.allow` adds extra `METHOD /path` entries, each with `{repo}`, to a default
    /// Proxy rule for these Boxes only. A new workflow has none when left out; when you replace
    /// one, left out keeps its config and an empty string clears it.
    pub(super) config: Option<String>,
    /// What each Box's agent is asked to do.
    pub(super) prompt: String,
    /// Minutes a run may take before it is stopped, 1 to 720; 180 for a new workflow when left
    /// out, and the stored value when you replace one.
    pub(super) timeout_minutes: Option<u32>,
    /// The plan a run walks, as an object with a `steps` list; see the tool description for how
    /// to write one. None when left out, and each run then delivers the prompt in one step.
    #[schemars(schema_with = "plan_schema")]
    pub(super) plan: Option<serde_json::Value>,
    /// The `revision` `list_workflows` showed for the workflow you are replacing. The call is
    /// refused when the workflow has changed since. Leave it out for a new workflow.
    pub(super) expected_revision: Option<u64>,
    /// False to store the workflow disabled in this same write, so no cron fires before you
    /// call `set_workflow_enabled`; true to enable it. Left out, a new workflow is enabled and a
    /// replaced one keeps its state.
    pub(super) enabled: Option<bool>,
    /// True to also fire a run when a pull request on the first repo is opened, reopened,
    /// marked ready for review or pushed to. The run gets the pull request in its env. Left
    /// out, a new workflow is false and a replaced one keeps its value.
    pub(super) on_pull_request: Option<bool>,
    /// Gates each run's Box must pass before the run counts as delivered, in the order they
    /// pass, in place of a plan; see the tool description. Left out, a new workflow has none and
    /// a replaced one keeps its gates; an empty list clears them. A workflow has a plan or gates,
    /// never both.
    pub(super) gates: Option<Vec<GateDoc>>,
    /// The outcomes a run may end in, sent with `gates`; see the tool description. Left out, a
    /// new workflow's run ends in `done` once every gate passes and a replaced one keeps its
    /// outcomes; an empty list clears them. A run's `verdict` in `list_workflow_runs` names
    /// the outcome it found.
    pub(super) outcomes: Option<Vec<moor_core::gate::OutcomeDoc>>,
}

/// A field that tells null from absent: absent stays `None` through `default`, and null is
/// `Some(None)`.
pub(super) fn some<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// The JSON Schema of a plan, which `add_workflow` checks the argument against.
fn plan_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    generator.subschema_for::<Option<PlanDoc>>()
}

/// Arguments of the tools that name one workflow.
#[derive(Deserialize, JsonSchema)]
pub(super) struct NameArgs {
    /// The workflow's name, as `list_workflows` shows it.
    pub(super) name: String,
}

/// Arguments of `set_workflow_enabled`.
#[derive(Deserialize, JsonSchema)]
pub(super) struct EnableArgs {
    /// The workflow's name, as `list_workflows` shows it.
    pub(super) name: String,
    /// True to fire from now on, false to stop firing.
    pub(super) enabled: bool,
}

/// What the owner answers.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum AnswerArg {
    /// Go on to the step after the approve step.
    Yes,
    /// Take the approve step's fail edge, or end the run declined when it has none.
    No,
}

/// Arguments of `answer_run`.
#[derive(Deserialize, JsonSchema)]
pub(super) struct AnswerArgs {
    /// The workflow's name, as `list_workflows` shows it.
    pub(super) workflow: String,
    /// The pull request whose run to answer, `OWNER/NAME#NUMBER`, as `list_workflow_runs`
    /// shows it. Needed when several runs of the workflow wait; left out, it answers the run for
    /// no pull request, or the only one that waits.
    pub(super) pull: Option<String>,
    /// `yes` or `no`.
    pub(super) answer: AnswerArg,
    /// What to tell the agent along with the answer; none when left out.
    pub(super) note: Option<String>,
}

/// Arguments of `run_workflow`.
#[derive(Deserialize, JsonSchema)]
pub(super) struct RunNowArgs {
    /// The workflow's name, as `list_workflows` shows it.
    pub(super) name: String,
    /// The `revision` `list_workflows` showed for the workflow. The call is refused when the
    /// workflow has changed since, so the run tries the plan you reviewed.
    pub(super) expected_revision: Option<u64>,
    /// A pull request on the workflow's first repo, `OWNER/NAME#NUMBER`, to run for at its
    /// current head. Needs a GitHub App in the controller config.
    pub(super) pull: Option<String>,
}

/// Arguments of `list_workflow_runs`.
#[derive(Deserialize, JsonSchema)]
pub(super) struct RunsArgs {
    /// The `next` of the previous call, to get only what changed since; everything when left out.
    pub(super) after: Option<i64>,
    /// Most changes to return, up to 200; 50 when left out.
    pub(super) limit: Option<u32>,
    /// A workflow name, as `list_workflows` shows it, to read only that workflow's changes;
    /// every workflow's when left out.
    pub(super) workflow: Option<String>,
    /// A pull request, `OWNER/NAME#NUMBER`, to read only the changes of its runs; runs for any
    /// pull request or none when left out.
    pub(super) pull: Option<String>,
}

/// Arguments of `get_workflow_run`.
#[derive(Deserialize, JsonSchema)]
pub(super) struct RunArgs {
    /// The workflow's name, as `list_workflows` shows it.
    pub(super) workflow: String,
    /// The run's `id`, as a row of `list_workflow_runs` shows it.
    pub(super) run: i64,
}

/// The pull request `arg` names, or why it names none.
pub(super) fn pull_arg(arg: Option<&str>) -> Result<Option<PullTarget>, String> {
    arg.map(PullTarget::parse)
        .transpose()
        .map_err(|e| e.to_string())
}
