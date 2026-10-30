//! Store rows to the core views.

use std::collections::HashMap;

use moor_core::activity::AgentState;
use moor_core::config::UserConfig;
use moor_core::domain::boxes::BoxState;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::SleepKind;
use moor_core::view::{AgentView, BoxSleepView, BoxView, DriverView, EventView, RunView};
use moor_core::ProviderId;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::cx::Cx;
use crate::ops::common::activity::Reading;
use crate::store::{BoxRow, DriverRow, EventRow, RunRow};

#[cfg(test)]
mod tests;

/// Format a timestamp as RFC3339 UTC with second precision.
///
/// A timestamp outside years 0..=9999 (a corrupt row) renders as its raw
/// millisecond count in decimal.
pub(crate) fn rfc3339(ts: Ts) -> String {
    let ms = ts.as_millis();
    OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000))
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_else(|| ms.to_string())
}

/// Map a reading to the agent view; none when the Box has no agent.
pub(crate) fn agent_view(reading: &Reading) -> Option<AgentView> {
    if let Some(failure) = &reading.launch_failure {
        return Some(AgentView {
            launched: false,
            pane: reading.pane.as_str().to_owned(),
            state: "failed".to_owned(),
            source: "launch".to_owned(),
            exit_code: None,
            question: None,
            options: Vec::new(),
            waiting_on: Vec::new(),
            reason: Some(failure.clone()),
        });
    }
    let activity = reading.activity.as_ref()?;
    let (exit_code, question, options) = match &activity.state {
        AgentState::Exited { code } => (Some(*code), None, Vec::new()),
        AgentState::NeedsInput { question, options } => (None, question.clone(), options.clone()),
        _ => (None, None, Vec::new()),
    };
    let waiting_on = match &activity.state {
        AgentState::Waiting { on } => on.clone(),
        _ => Vec::new(),
    };
    Some(AgentView {
        launched: true,
        pane: reading.pane.as_str().to_owned(),
        state: activity.state.as_str().to_owned(),
        source: activity.source.as_str().to_owned(),
        exit_code,
        question,
        options,
        waiting_on,
        reason: None,
    })
}

/// The handle to show: live, or the attempted one once the Box failed.
fn box_handle(b: &DriverRow) -> Option<String> {
    let h = match b.state {
        BoxState::Gone | BoxState::Deleted => None,
        BoxState::Failed | BoxState::ReleaseFailed => {
            b.handle.as_ref().or(b.attempted_handle.as_ref())
        }
        _ => b.handle.as_ref(),
    };
    h.map(|h| h.as_str().to_owned())
}

/// Map a Box row to its view.
fn driver_view(b: &DriverRow, config: &UserConfig) -> DriverView {
    DriverView {
        state: b.state,
        provider: b.provider.as_str().to_owned(),
        handle: box_handle(b),
        workdir: b.workdir.clone(),
        egress: b.egress.clone(),
        cpus: b.delivered.map(|d| d.cpus),
        mem_mib: b.delivered.map(|d| d.mem_mib),
        asleep_since: (b.state == BoxState::Asleep).then(|| rfc3339(b.updated_at)),
        sleep: BoxSleepView {
            after: Some(config.box_.sleep_after_label()),
            reason: b.sleep_reason.clone(),
        },
    }
}

/// Map an in-flight run to its view.
fn run_view(r: &RunRow) -> RunView {
    RunView {
        id: r.id.get(),
        argv: r.argv.clone(),
        started_at: rfc3339(r.started_at),
    }
}

/// Map a Box row to the shared view; `agent` and `run` stay empty.
pub(crate) fn box_view(row: &BoxRow, config: &UserConfig) -> BoxView {
    box_view_with_run(row, None, config)
}

/// Map a Box row to the shared view, showing `run` when one is in flight.
///
/// `agent` stays empty.
pub(crate) fn box_view_with_run(
    row: &BoxRow,
    run: Option<&RunRow>,
    config: &UserConfig,
) -> BoxView {
    BoxView {
        id: row.id.get(),
        name: row.name.as_str().to_owned(),
        repo: row.repo.as_ref().map(|r| r.repo.clone()),
        issue: row.issue.clone(),
        prompt: row.prompt.clone(),
        lifecycle: row.lifecycle,
        session_state: row.session,
        state_reason: row.state_reason.clone(),
        base: row.base.clone(),
        branch: row.branch.clone(),
        workspace: row.workspace_id.clone(),
        driver: row.boxed.as_ref().map(|b| driver_view(b, config)),
        agent: None,
        run: run.map(run_view),
        updated_at: rfc3339(row.updated_at),
        attention: Vec::new(),
        held_machine: None,
    }
}

/// Drop the idle-sleep label when the Box's provider cannot sleep.
///
/// A provider whose capabilities cannot be read keeps the label.
pub(crate) fn hide_unsupported_sleep(view: &mut BoxView, cx: &Cx) {
    hide_unsupported_sleep_cached(view, cx, &mut HashMap::new());
}

/// Like [`hide_unsupported_sleep`], reading each provider's capabilities once per `cache`.
pub(crate) fn hide_unsupported_sleep_cached(
    view: &mut BoxView,
    cx: &Cx,
    cache: &mut HashMap<String, bool>,
) {
    hide_unsupported_box_sleep(view.driver.as_mut(), cx, cache);
}

/// Drop the idle-sleep label from `b` when its provider cannot sleep.
pub(crate) fn hide_unsupported_box_sleep(
    b: Option<&mut DriverView>,
    cx: &Cx,
    cache: &mut HashMap<String, bool>,
) {
    let Some(b) = b else {
        return;
    };
    let cannot_sleep = *cache.entry(b.provider.clone()).or_insert_with(|| {
        let caps = cx.driver().caps(&ProviderId::new(b.provider.clone()));
        matches!(caps, Ok(c) if c.sleep == SleepKind::None)
    });
    if cannot_sleep {
        b.sleep.after = None;
    }
}

/// Map an event row to its view.
pub(crate) fn event_view(row: &EventRow) -> EventView {
    EventView {
        id: row.id,
        kind: row.kind.clone(),
        payload: row.payload.clone(),
        ts: rfc3339(row.ts),
    }
}
