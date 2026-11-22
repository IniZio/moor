//! Each user's workflows and the record of their firings.

use std::time::Duration;

use moor_core::gate::Gates;
use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::workflow::{
    Plan, TimeoutMinutes, Workflow, WorkflowId, WorkflowName, WorkflowOrigin, WorkflowSchedule,
    WorkflowState,
};
use moor_schema::tables::{WorkflowRuns, Workflows};
use sea_query::{Cond, Expr, ExprTrait as _, Func, Order, Query};

pub(crate) use claim::OpenClaim;
pub(in crate::store) use history::append_to_open_run;
pub(crate) use put::Put;
pub(in crate::store) use runs::run_of_box_in;
pub(crate) use runs::RunOfBox;

mod changes;
mod claim;
mod history;
mod list;
mod notice;
mod pointer;
mod put;
mod row;
mod runs;
mod subject;
mod waiting;

use crate::store::db::{self, DbExt as _, OptionalExtension as _};
use crate::store::{Store, StoreError};
use row::{STARTED, STARTING, WAITING};

/// A `workflows` row as read, before it is parsed.
struct WorkflowRow {
    /// `id`.
    id: i64,
    /// `user`.
    user: i64,
    /// `name`.
    name: String,
    /// `cron`.
    cron: Option<String>,
    /// `timezone`.
    timezone: Option<String>,
    /// `repos`, as JSON.
    repos: String,
    /// `prompt`.
    prompt: String,
    /// `enabled_at`.
    enabled_at: Option<i64>,
    /// `timeout_minutes`.
    timeout_minutes: i64,
    /// `created_at`.
    created_at: i64,
    /// `updated_at`.
    updated_at: i64,
    /// `origin_front`.
    origin_front: Option<String>,
    /// `origin_key`.
    origin_key: Option<String>,
    /// `config`.
    config: Option<String>,
    /// `revision`.
    revision: i64,
    /// `plan`, as JSON.
    plan: Option<String>,
    /// `on_pull_request`.
    on_pull_request: i64,
    /// `gates`, as JSON.
    gates: Option<String>,
}

/// The columns of a `workflows` row, in the order [`read_workflow`] reads them.
const WORKFLOW_COLS: [Workflows; 18] = [
    Workflows::Id,
    Workflows::User,
    Workflows::Name,
    Workflows::Cron,
    Workflows::Timezone,
    Workflows::Repos,
    Workflows::Prompt,
    Workflows::EnabledAt,
    Workflows::TimeoutMinutes,
    Workflows::CreatedAt,
    Workflows::UpdatedAt,
    Workflows::OriginFront,
    Workflows::OriginKey,
    Workflows::Config,
    Workflows::Revision,
    Workflows::Plan,
    Workflows::OnPullRequest,
    Workflows::Gates,
];

/// The store error for a stored value that does not parse.
pub(super) fn corrupt(what: &str, e: impl std::fmt::Display) -> StoreError {
    StoreError::Corrupt(format!("workflows {what}: {e}"))
}

/// Reads one `workflows` row.
fn read_workflow(r: &db::Row) -> Result<WorkflowRow, db::Error> {
    Ok(WorkflowRow {
        id: r.get(0)?,
        user: r.get(1)?,
        name: r.get(2)?,
        cron: r.get(3)?,
        timezone: r.get(4)?,
        repos: r.get(5)?,
        prompt: r.get(6)?,
        enabled_at: r.get(7)?,
        timeout_minutes: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
        origin_front: r.get(11)?,
        origin_key: r.get(12)?,
        config: r.get(13)?,
        revision: r.get(14)?,
        plan: r.get(15)?,
        on_pull_request: r.get(16)?,
        gates: r.get(17)?,
    })
}

/// The workflow a stored row holds. The schedule was checked against the floor when it was
/// stored, and a later change of the floor must not make it unreadable.
fn workflow_of(row: WorkflowRow) -> Result<Workflow, StoreError> {
    let timeout = u32::try_from(row.timeout_minutes)
        .map_err(|e| corrupt("timeout", e))
        .and_then(|m| TimeoutMinutes::new(m).map_err(|e| corrupt("timeout", e)))?;
    Ok(Workflow {
        id: WorkflowId::new(row.id),
        owner: UserId::new(row.user),
        name: WorkflowName::parse(&row.name).map_err(|e| corrupt("name", e))?,
        schedule: match (&row.cron, &row.timezone) {
            (Some(cron), Some(zone)) => Some(
                WorkflowSchedule::parse_with_min_interval(cron, zone, Duration::ZERO)
                    .map_err(|e| corrupt("schedule", e))?,
            ),
            (None, None) => None,
            _ => return Err(corrupt("schedule", "cron and timezone are set together")),
        },
        repos: serde_json::from_str(&row.repos).map_err(|e| corrupt("repos", e))?,
        config: row.config,
        prompt: row.prompt.clone(),
        state: row
            .enabled_at
            .map_or(WorkflowState::Disabled, |ms| WorkflowState::Enabled {
                since: Ts::from_millis(ms),
            }),
        timeout,
        origin: row.origin_front.map(|front| WorkflowOrigin {
            front,
            key: row.origin_key,
        }),
        created_at: Ts::from_millis(row.created_at),
        updated_at: Ts::from_millis(row.updated_at),
        plan: match &row.plan {
            Some(json) => Plan::parse(json, &row.prompt).map_err(|e| {
                corrupt(
                    "plan",
                    e.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; "),
                )
            })?,
            None => Plan::single(&row.prompt),
        },
        revision: u64::try_from(row.revision).map_err(|e| corrupt("revision", e))?,
        on_pull_request: row.on_pull_request != 0,
        gates: row
            .gates
            .as_deref()
            .map(Gates::from_json)
            .transpose()
            .map_err(|e| corrupt("gates", e))?,
    })
}

impl Store {
    /// `owner`'s workflow `name`.
    pub(crate) fn workflow(
        &self,
        owner: UserId,
        name: &WorkflowName,
    ) -> Result<Option<Workflow>, StoreError> {
        let row = self
            .reader()?
            .row(
                Query::select()
                    .columns(WORKFLOW_COLS)
                    .from(Workflows::Table)
                    .and_where(Expr::col(Workflows::User).eq(owner.get()))
                    .and_where(Expr::col(Workflows::Name).eq(name.as_str())),
                read_workflow,
            )
            .optional()?;
        row.map(workflow_of).transpose()
    }

    /// `owner`'s workflows, by name.
    pub(crate) fn workflows(&self, owner: UserId) -> Result<Vec<Workflow>, StoreError> {
        let reader = self.reader()?;
        let rows = reader.rows(
            Query::select()
                .columns(WORKFLOW_COLS)
                .from(Workflows::Table)
                .and_where(Expr::col(Workflows::User).eq(owner.get()))
                .order_by(Workflows::Name, Order::Asc),
            read_workflow,
        )?;
        rows.map(|row| workflow_of(row?)).collect()
    }

    /// Every workflow of every owner the tick has work for, by id: each enabled one, and each
    /// disabled one with a run still open, which settles though it no longer fires.
    pub(crate) fn ticked_workflows(&self) -> Result<Vec<Workflow>, StoreError> {
        let reader = self.reader()?;
        let open = Query::select()
            .column(WorkflowRuns::Workflow)
            .from(WorkflowRuns::Table)
            .and_where(Expr::col(WorkflowRuns::Outcome).is_in([STARTING, STARTED, WAITING]))
            .take();
        let rows = reader.rows(
            Query::select()
                .columns(WORKFLOW_COLS)
                .from(Workflows::Table)
                .cond_where(
                    Cond::any()
                        .add(Expr::col(Workflows::EnabledAt).is_not_null())
                        .add(Expr::col(Workflows::Id).in_subquery(open)),
                )
                .order_by(Workflows::Id, Order::Asc),
            read_workflow,
        )?;
        rows.map(|row| workflow_of(row?)).collect()
    }

    /// Every workflow of every owner whose first repo is `repo`, compared as GitHub does,
    /// without case, by id.
    pub(crate) fn workflows_on(&self, repo: &ForgeRepo) -> Result<Vec<Workflow>, StoreError> {
        let reader = self.reader()?;
        let rows = reader.rows(
            Query::select()
                .columns(WORKFLOW_COLS)
                .from(Workflows::Table)
                .order_by(Workflows::Id, Order::Asc),
            read_workflow,
        )?;
        let mut found = Vec::new();
        for row in rows {
            let w = workflow_of(row?)?;
            let first = w.repos.first().is_some_and(|f| {
                f.owner.eq_ignore_ascii_case(&repo.owner) && f.name.eq_ignore_ascii_case(&repo.name)
            });
            if first {
                found.push(w);
            }
        }
        Ok(found)
    }

    /// Workflow `id`, whoever owns it.
    pub(crate) fn workflow_by_id(&self, id: WorkflowId) -> Result<Option<Workflow>, StoreError> {
        let row = self
            .reader()?
            .row(
                Query::select()
                    .columns(WORKFLOW_COLS)
                    .from(Workflows::Table)
                    .and_where(Expr::col(Workflows::Id).eq(id.get())),
                read_workflow,
            )
            .optional()?;
        row.map(workflow_of).transpose()
    }

    /// Deletes `owner`'s workflow `name` with its runs; true when there was one.
    pub(crate) fn delete_workflow(
        &self,
        owner: UserId,
        name: &WorkflowName,
    ) -> Result<bool, StoreError> {
        let gone = self.writer().run(
            Query::delete()
                .from_table(Workflows::Table)
                .and_where(Expr::col(Workflows::User).eq(owner.get()))
                .and_where(Expr::col(Workflows::Name).eq(name.as_str())),
        )?;
        Ok(gone == 1)
    }

    /// Enables or disables `owner`'s workflow `name`; true when there is one. Enabling counts
    /// from now unless it was enabled already, and clears the reason the controller disabled it
    /// for. Disabling leaves that reason.
    pub(crate) fn set_workflow_enabled(
        &self,
        owner: UserId,
        name: &WorkflowName,
        enabled: bool,
    ) -> Result<bool, StoreError> {
        let now = self.now().as_millis();
        let mut update = Query::update();
        update
            .table(Workflows::Table)
            .and_where(Expr::col(Workflows::User).eq(owner.get()))
            .and_where(Expr::col(Workflows::Name).eq(name.as_str()));
        if enabled {
            update.value(
                Workflows::EnabledAt,
                Func::coalesce([Expr::col(Workflows::EnabledAt), Expr::value(now)]),
            );
        } else {
            update.value(Workflows::EnabledAt, Option::<i64>::None);
        }
        update.value(Workflows::UpdatedAt, now);
        Ok(self.writer().run(&mut update)? == 1)
    }
}
