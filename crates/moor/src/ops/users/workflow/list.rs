//! `workflow-list`: a user's workflows as shown.

use moor_core::gate::Gates;
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::view::workflow::{LastRunView, PlanMakingView, WorkflowListValue, WorkflowView};
use moor_core::workflow::{PlanDraft, PromptHash, Workflow};

use super::next_due;
use super::stream::run_view;
use crate::error::CommandError;
use crate::ops::users::to_value;
use crate::store::Store;

/// `workflow-list`.
pub(super) fn list(
    store: &Store,
    user: UserId,
    now: Ts,
) -> Result<serde_json::Value, CommandError> {
    let mut workflows = Vec::new();
    let mut drafts: std::collections::HashMap<_, _> = store.plan_drafts()?.into_iter().collect();
    for w in store.workflows(user)? {
        let last_run = match store.last_workflow_run(w.id)? {
            Some(run) => Some(run_view(store, w.name.as_str(), &run)?),
            None => None,
        };
        let making = drafts.remove(&w.id);
        workflows.push(view(&w, now, (last_run, making)));
    }
    to_value(&WorkflowListValue { workflows })
}

/// `w` as shown.
fn view(
    w: &Workflow,
    now: Ts,
    (last_run, plan_making): (Option<LastRunView>, Option<PlanDraft>),
) -> WorkflowView {
    let enabled = matches!(w.state, moor_core::workflow::WorkflowState::Enabled { .. });
    WorkflowView {
        name: w.name.to_string(),
        cron: w.schedule.as_ref().map(|s| s.expr().to_owned()),
        timezone: w.schedule.as_ref().map(|s| s.zone().to_owned()),
        repos: w.repos.iter().map(ToString::to_string).collect(),
        config: w.config.clone(),
        prompt: w.prompt.clone(),
        enabled,
        timeout_minutes: w.timeout.minutes(),
        next_due: enabled
            .then(|| next_due(w.schedule.as_ref(), now))
            .flatten(),
        plan: (!w.plan.is_single_of(&w.prompt))
            .then(|| serde_json::to_value(w.plan.to_doc()).ok())
            .flatten(),
        plan_stale: w.plan.made_from != PromptHash::of(&w.prompt),
        plan_making: plan_making.map(|d| PlanMakingView {
            prompt: d.prompt,
            box_name: d.box_name,
            since: d.since,
            failed: d.failed,
        }),
        revision: w.revision,
        on_pull_request: w.on_pull_request,
        gates: w.gates.as_ref().map(Gates::docs),
        outcomes: w
            .gates
            .as_ref()
            .map(Gates::outcome_docs)
            .filter(|o| !o.is_empty()),
        last_run,
    }
}
