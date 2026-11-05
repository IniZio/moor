//! Each user's workflows and the record of their firings.

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "read by the firing loop, not yet wired")
)]

use std::time::Duration;

use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::workflow::{
    TimeoutMinutes, Workflow, WorkflowDef, WorkflowId, WorkflowName, WorkflowSchedule,
    WorkflowState,
};
use moor_schema::tables::Workflows;
use sea_query::{Expr, ExprTrait as _, Func, Order, Query};

mod runs;

use crate::store::db::{self, Db, DbExt as _, OptionalExtension as _};
use crate::store::{Store, StoreError};

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
}

/// The columns of a `workflows` row, in the order [`read_workflow`] reads them.
const WORKFLOW_COLS: [Workflows; 12] = [
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
        prompt: row.prompt,
        state: row.enabled_at.map_or(
            WorkflowState::Disabled {
                reason: row.disabled_reason,
            },
            |ms| WorkflowState::Enabled {
                since: Ts::from_millis(ms),
            },
        ),
        timeout,
        created_at: Ts::from_millis(row.created_at),
        updated_at: Ts::from_millis(row.updated_at),
    })
}

/// Inserts `def` as `owner`'s workflow and returns its id.
fn insert_workflow(
    conn: &dyn Db,
    owner: UserId,
    def: &WorkflowDef,
    repos: String,
    now: i64,
) -> Result<i64, StoreError> {
    Ok(conn.row(
        Query::insert()
            .into_table(Workflows::Table)
            .columns([
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
            ])
            .values_panic([
                owner.get().into(),
                def.name.as_str().into(),
                def.schedule.expr().into(),
                def.schedule.zone().into(),
                repos.into(),
                def.prompt.as_str().into(),
                def.enabled.then_some(now).into(),
                i64::from(def.timeout.minutes()).into(),
                now.into(),
                now.into(),
            ])
            .returning_col(Workflows::Id),
        |r| r.get(0),
    )?)
}

impl Store {
    /// Stores `def` as `owner`'s workflow of its name, replacing the one already there, and
    /// returns its id. A replaced workflow keeps its id, its runs and its creation time; it
    /// counts as enabled since now unless it was enabled already. Enabling it clears the reason
    /// the controller disabled it for; storing it disabled leaves that reason.
    pub(crate) fn put_workflow(
        &self,
        owner: UserId,
        def: &WorkflowDef,
    ) -> Result<WorkflowId, StoreError> {
        let now = self.now().as_millis();
        let repos = serde_json::to_string(&def.repos)?;
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let found: Option<(i64, Option<i64>, Option<String>)> = tx
            .row(
                Query::select()
                    .columns([
                        Workflows::Id,
                        Workflows::EnabledAt,
                        Workflows::DisabledReason,
                    ])
                    .from(Workflows::Table)
                    .and_where(Expr::col(Workflows::User).eq(owner.get()))
                    .and_where(Expr::col(Workflows::Name).eq(def.name.as_str())),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let id = if let Some((id, was_enabled_at, was_reason)) = found {
            let enabled_at = def.enabled.then_some(was_enabled_at.unwrap_or(now));
            let reason = if def.enabled { None } else { was_reason };
            tx.run(
                Query::update()
                    .table(Workflows::Table)
                    .value(Workflows::Cron, def.schedule.expr())
                    .value(Workflows::Timezone, def.schedule.zone())
                    .value(Workflows::Repos, repos)
                    .value(Workflows::Prompt, def.prompt.as_str())
                    .value(Workflows::EnabledAt, enabled_at)
                    .value(Workflows::DisabledReason, reason)
                    .value(Workflows::TimeoutMinutes, i64::from(def.timeout.minutes()))
                    .value(Workflows::UpdatedAt, now)
                    .and_where(Expr::col(Workflows::Id).eq(id)),
            )?;
            id
        } else {
            insert_workflow(&tx, owner, def, repos, now)?
        };
        tx.commit()?;
        drop(writer);
        Ok(WorkflowId::new(id))
    }

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
        let changed = self.writer().run(
            Query::update()
                .table(Workflows::Table)
                .value(Workflows::EnabledAt, Option::<i64>::None)
                .value(Workflows::DisabledReason, reason)
                .value(Workflows::UpdatedAt, now)
                .and_where(Expr::col(Workflows::Id).eq(id.get()))
                .and_where(Expr::col(Workflows::EnabledAt).is_not_null()),
        )?;
        Ok(changed == 1)
    }
}
