//! Claims an earlier process left behind, settled without starting a Box.

use moor_core::decide::workflow::{run_verdict, RunVerdict};
use moor_core::workflow::{box_name, RunEnd, RunState};

use super::gated;
use super::settle::broken_detail;
use crate::cx::Cx;
use crate::ops::common::quiet::box_facts;

/// Settles each claim an earlier process left, never starting a Box: one whose Box is open
/// becomes its run, any other is recorded as refused. Returns what to log.
pub(crate) fn resolve_interrupted(cx: &Cx) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(store) = cx.store() else { return out };
    let claims = store.starting_workflow_runs().unwrap_or_default();
    for run in claims {
        let workflow = store.workflow_by_id(run.workflow).ok().flatten();
        let found = workflow
            .as_ref()
            .and_then(|w| box_name(w.id, w.name.as_str(), run.due_at, &run.cause).ok())
            .and_then(|name| store.open_box_by_name(&name).ok().flatten());
        let now = cx.clock().now();
        let state = match (found, workflow) {
            (Some(row), Some(w)) => {
                match box_facts(store, &row).map(|f| run_verdict(Some(&f), now)) {
                    Ok(RunVerdict::Broken) => RunState::Done(RunEnd::Refused {
                        detail: broken_detail(store, &row),
                    }),
                    _ => RunState::Running {
                        box_id: row.id,
                        at: gated::first_step(
                            store.box_gates(row.id).ok().flatten().as_ref(),
                            store.run_plan(run.id, &w.prompt).map_or_else(
                                |_| w.plan.first().id.clone(),
                                |p| p.first().id.clone(),
                            ),
                        ),
                        try_n: 1,
                    },
                }
            }
            _ => RunState::Done(RunEnd::Refused {
                detail: "the controller stopped before the Box started".to_owned(),
            }),
        };
        if let Err(e) = store.resolve_workflow_start(run.id, &state, now) {
            out.push(format!(
                "could not settle a claimed run of {}: {e}",
                run.repo
            ));
        }
    }
    out
}
