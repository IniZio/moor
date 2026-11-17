//! Storing a workflow: the write, the plan with it, and the revision check.

use moor_core::user::UserId;
use moor_core::workflow::{Plan, WorkflowDef, WorkflowId};
use moor_schema::tables::Workflows;
use sea_query::{Expr, ExprTrait as _, Query};

use super::corrupt;
use crate::store::db::{Db, DbExt as _, OptionalExtension as _};
use crate::store::{Store, StoreError};

/// The stored workflow a put replaces.
struct Existing {
    /// `id`.
    id: i64,
    /// `enabled_at`.
    enabled_at: Option<i64>,
    /// `disabled_reason`.
    reason: Option<String>,
    /// `revision`.
    revision: u64,
}

/// The workflow `def` replaces, read in `tx`.
fn existing(tx: &dyn Db, owner: UserId, def: &WorkflowDef) -> Result<Option<Existing>, StoreError> {
    let found: Option<(i64, Option<i64>, Option<String>, i64)> = tx
        .row(
            Query::select()
                .columns([
                    Workflows::Id,
                    Workflows::EnabledAt,
                    Workflows::DisabledReason,
                    Workflows::Revision,
                ])
                .from(Workflows::Table)
                .and_where(Expr::col(Workflows::User).eq(owner.get()))
                .and_where(Expr::col(Workflows::Name).eq(def.name.as_str())),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    found
        .map(|(id, enabled_at, reason, revision)| {
            Ok(Existing {
                id,
                enabled_at,
                reason,
                revision: u64::try_from(revision).map_err(|e| corrupt("revision", e))?,
            })
        })
        .transpose()
}

/// What a write of a workflow did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Put {
    /// Stored; the workflow's id and the revision it now has.
    Written {
        /// The database id.
        id: WorkflowId,
        /// The revision after the write, from 0 for a workflow just made.
        revision: u64,
    },
    /// Refused: the stored workflow is not at the revision the writer read.
    Stale {
        /// The stored revision; none when there is no such workflow.
        current: Option<u64>,
    },
}

/// Inserts `def` as `owner`'s workflow and returns its id.
fn insert_workflow(
    conn: &dyn Db,
    owner: UserId,
    def: &WorkflowDef,
    (repos, also, plan): (String, String, Option<String>),
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
                Workflows::Also,
                Workflows::Prompt,
                Workflows::EnabledAt,
                Workflows::TimeoutMinutes,
                Workflows::CreatedAt,
                Workflows::UpdatedAt,
                Workflows::OriginFront,
                Workflows::OriginKey,
                Workflows::Config,
                Workflows::Plan,
            ])
            .values_panic([
                owner.get().into(),
                def.name.as_str().into(),
                def.schedule.expr().into(),
                def.schedule.zone().into(),
                repos.into(),
                also.into(),
                def.prompt.as_str().into(),
                def.enabled.then_some(now).into(),
                i64::from(def.timeout.minutes()).into(),
                now.into(),
                now.into(),
                def.origin.as_ref().map(|o| o.front.as_str()).into(),
                def.origin.as_ref().and_then(|o| o.key.as_deref()).into(),
                def.config.as_deref().into(),
                plan.into(),
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
    #[cfg(test)]
    pub(crate) fn put_workflow(
        &self,
        owner: UserId,
        def: &WorkflowDef,
    ) -> Result<WorkflowId, StoreError> {
        match self.put_workflow_at(owner, def, None)? {
            Put::Written { id, .. } => Ok(id),
            Put::Stale { .. } => Err(StoreError::Invalid(
                "a write that names no revision was refused as stale".to_owned(),
            )),
        }
    }

    /// Stores `def` as `put_workflow` does, writing only when the stored workflow is at revision `expected`,
    /// if one is given. The check and the write share one transaction.
    pub(crate) fn put_workflow_at(
        &self,
        owner: UserId,
        def: &WorkflowDef,
        expected: Option<u64>,
    ) -> Result<Put, StoreError> {
        let now = self.now().as_millis();
        let repos = serde_json::to_string(&def.repos)?;
        let also = serde_json::to_string(&def.also)?;
        let plan = def.plan.as_ref().map(Plan::to_json).transpose()?;
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let origin = def.origin.as_ref();
        let found = existing(&tx, owner, def)?;
        let current = found.as_ref().map(|f| f.revision);
        if expected.is_some_and(|want| current != Some(want)) {
            return Ok(Put::Stale { current });
        }
        let (id, revision) = if let Some(Existing {
            id,
            enabled_at: was_enabled_at,
            reason: was_reason,
            ..
        }) = found
        {
            let enabled_at = def.enabled.then_some(was_enabled_at.unwrap_or(now));
            let reason = if def.enabled { None } else { was_reason };
            tx.run(
                Query::update()
                    .table(Workflows::Table)
                    .value(Workflows::Cron, def.schedule.expr())
                    .value(Workflows::Timezone, def.schedule.zone())
                    .value(Workflows::Repos, repos)
                    .value(Workflows::Also, also)
                    .value(Workflows::Config, def.config.as_deref())
                    .value(Workflows::Prompt, def.prompt.as_str())
                    .value(Workflows::Plan, plan)
                    .value(Workflows::EnabledAt, enabled_at)
                    .value(Workflows::DisabledReason, reason)
                    .value(Workflows::TimeoutMinutes, i64::from(def.timeout.minutes()))
                    .value(Workflows::UpdatedAt, now)
                    .value(
                        Workflows::Revision,
                        Expr::col(Workflows::Revision).add(1_i64),
                    )
                    .value(Workflows::OriginFront, origin.map(|o| o.front.as_str()))
                    .value(Workflows::OriginKey, origin.and_then(|o| o.key.as_deref()))
                    .and_where(Expr::col(Workflows::Id).eq(id)),
            )?;
            (id, current.unwrap_or_default() + 1)
        } else {
            (
                insert_workflow(&tx, owner, def, (repos, also, plan), now)?,
                0,
            )
        };
        tx.commit()?;
        drop(writer);
        Ok(Put::Written {
            id: WorkflowId::new(id),
            revision,
        })
    }
}
