//! Each user's workflows and the record of their firings.

use std::time::Duration;

use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::workflow::{
    Plan, TimeoutMinutes, Workflow, WorkflowId, WorkflowName, WorkflowOrigin, WorkflowSchedule,
    WorkflowState,
};
use moor_schema::tables::{WorkflowRuns, Workflows};
use sea_query::{Cond, Expr, ExprTrait as _, Func, Order, Query};

pub(crate) use claim::ManualClaim;
pub(crate) use put::Put;

mod changes;
mod claim;
mod history;
mod list;
mod notice;
mod pointer;
mod put;
mod row;
mod runs;
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
    cron: String,
    /// `timezone`.
    timezone: String,
    /// `repos`, as JSON.
    repos: String,
    /// `also`, as JSON.
    also: String,
    /// `prompt`.
    prompt: String,
    /// `enabled_at`.
    enabled_at: Option<i64>,
    /// `disabled_reason`.
    disabled_reason: Option<String>,
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
    Workflows::DisabledReason,
    Workflows::TimeoutMinutes,
    Workflows::CreatedAt,
    Workflows::UpdatedAt,
    Workflows::OriginFront,
    Workflows::OriginKey,
    Workflows::Also,
    Workflows::Config,
    Workflows::Revision,
    Workflows::Plan,
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
        disabled_reason: r.get(8)?,
        timeout_minutes: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        origin_front: r.get(12)?,
        origin_key: r.get(13)?,
        also: r.get(14)?,
        config: r.get(15)?,
        revision: r.get(16)?,
        plan: r.get(17)?,
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
        schedule: WorkflowSchedule::parse_with_min_interval(
            &row.cron,
            &row.timezone,
            Duration::ZERO,
        )
        .map_err(|e| corrupt("schedule", e))?,
        repos: serde_json::from_str(&row.repos).map_err(|e| corrupt("repos", e))?,
        also: serde_json::from_str(&row.also).map_err(|e| corrupt("also", e))?,
        config: row.config,
        prompt: row.prompt.clone(),
        state: row.enabled_at.map_or(
            WorkflowState::Disabled {
                reason: row.disabled_reason,
            },
            |ms| WorkflowState::Enabled {
                since: Ts::from_millis(ms),
            },
        ),
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
            update
                .value(
                    Workflows::EnabledAt,
                    Func::coalesce([Expr::col(Workflows::EnabledAt), Expr::value(now)]),
                )
                .value(Workflows::DisabledReason, Option::<String>::None);
        } else {
            update.value(Workflows::EnabledAt, Option::<i64>::None);
        }
        update.value(Workflows::UpdatedAt, now);
        Ok(self.writer().run(&mut update)? == 1)
    }

    /// Disables workflow `id` for `reason`; true when it was enabled. One its owner disabled is
    /// left as it is.
    pub(crate) fn disable_workflow(
        &self,
        id: WorkflowId,
        reason: &str,
    ) -> Result<bool, StoreError> {
        let now = self.now().as_millis();
        self.numbered(|tx, seq| {
            Ok(tx.run(
                Query::update()
                    .table(Workflows::Table)
                    .value(Workflows::EnabledAt, Option::<i64>::None)
                    .value(Workflows::DisabledReason, reason)
                    .value(Workflows::ChangeSeq, seq)
                    .value(Workflows::UpdatedAt, now)
                    .and_where(Expr::col(Workflows::Id).eq(id.get()))
                    .and_where(Expr::col(Workflows::EnabledAt).is_not_null()),
            )?)
        })
    }
}
