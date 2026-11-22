//! Storing a workflow: the write, the plan with it, and the revision check.

use moor_core::user::UserId;
use moor_core::workflow::{Plan, WorkflowDef, WorkflowId, WorkflowSchedule};
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
    /// `revision`.
    revision: u64,
}

/// The workflow `def` replaces, read in `tx`.
fn existing(tx: &dyn Db, owner: UserId, def: &WorkflowDef) -> Result<Option<Existing>, StoreError> {
    let found: Option<(i64, Option<i64>, i64)> = tx
        .row(
            Query::select()
                .columns([Workflows::Id, Workflows::EnabledAt, Workflows::Revision])
                .from(Workflows::Table)
                .and_where(Expr::col(Workflows::User).eq(owner.get()))
                .and_where(Expr::col(Workflows::Name).eq(def.name.as_str())),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    found
        .map(|(id, enabled_at, revision)| {
            Ok(Existing {
                id,
                enabled_at,
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

/// The JSON a workflow keeps of its gates; none for none.
fn gates_json(def: &WorkflowDef) -> Result<Option<String>, StoreError> {
    def.gates
        .as_ref()
        .map(|g| serde_json::to_string(&g.docs()))
        .transpose()
        .map_err(StoreError::from)
}

/// Inserts `def` as `owner`'s workflow and returns its id.
fn insert_workflow(
    conn: &dyn Db,
    owner: UserId,
    def: &WorkflowDef,
    (repos, plan): (String, Option<String>),
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
                Workflows::OriginFront,
                Workflows::OriginKey,
                Workflows::Config,
                Workflows::Plan,
                Workflows::OnPullRequest,
                Workflows::Gates,
            ])
            .values_panic([
                owner.get().into(),
                def.name.as_str().into(),
                def.schedule.as_ref().map(WorkflowSchedule::expr).into(),
                def.schedule.as_ref().map(WorkflowSchedule::zone).into(),
                repos.into(),
                def.prompt.as_str().into(),
                def.enabled.then_some(now).into(),
                i64::from(def.timeout.minutes()).into(),
                now.into(),
                now.into(),
                def.origin.as_ref().map(|o| o.front.as_str()).into(),
                def.origin.as_ref().and_then(|o| o.key.as_deref()).into(),
                def.config.as_deref().into(),
                plan.into(),
                i64::from(def.on_pull_request).into(),
                gates_json(def)?.into(),
            ])
            .returning_col(Workflows::Id),
        |r| r.get(0),
    )?)
}

/// Replaces the content of workflow `id` with `def`, at `enabled_at`
/// and time `now`, and raises its revision. The origin is written only when `def` has one.
fn replace_workflow(
    conn: &dyn Db,
    id: i64,
    def: &WorkflowDef,
    (repos, plan): (String, Option<String>),
    (enabled_at, now): (Option<i64>, i64),
) -> Result<(), StoreError> {
    let mut update = Query::update();
    update
        .table(Workflows::Table)
        .value(
            Workflows::Cron,
            def.schedule.as_ref().map(WorkflowSchedule::expr),
        )
        .value(
            Workflows::Timezone,
            def.schedule.as_ref().map(WorkflowSchedule::zone),
        )
        .value(Workflows::Repos, repos)
        .value(Workflows::Config, def.config.as_deref())
        .value(Workflows::Prompt, def.prompt.as_str())
        .value(Workflows::Plan, plan)
        .value(Workflows::OnPullRequest, i64::from(def.on_pull_request))
        .value(Workflows::Gates, gates_json(def)?)
        .value(Workflows::EnabledAt, enabled_at)
        .value(Workflows::TimeoutMinutes, i64::from(def.timeout.minutes()))
        .value(Workflows::UpdatedAt, now)
        .value(
            Workflows::Revision,
            Expr::col(Workflows::Revision).add(1_i64),
        )
        .and_where(Expr::col(Workflows::Id).eq(id));
    if let Some(origin) = &def.origin {
        update
            .value(Workflows::OriginFront, origin.front.as_str())
            .value(Workflows::OriginKey, origin.key.as_deref());
    }
    conn.run(&mut update)?;
    Ok(())
}

impl Store {
    /// Stores `def` as `owner`'s workflow of its name, replacing the one already there, and
    /// returns its id. A replaced workflow keeps its id, its runs and its creation time; it
    /// counts as enabled since now unless it was enabled already.
    #[cfg(test)]
    pub(crate) fn put_workflow(
        &self,
        owner: UserId,
        def: &WorkflowDef,
    ) -> Result<WorkflowId, StoreError> {
        match self.put_workflow_at(owner, def, None, false)? {
            Put::Written { id, .. } => Ok(id),
            Put::Stale { .. } => Err(StoreError::Invalid(
                "a write that names no revision was refused as stale".to_owned(),
            )),
        }
    }

    /// Stores `def` as `put_workflow` does, writing only when the stored workflow is at revision `expected`,
    /// if one is given. The check and the write share one transaction. A replace with
    /// `keep_state` leaves whether the workflow is enabled as stored.
    /// A `def` with no origin leaves the stored origin as it is.
    pub(crate) fn put_workflow_at(
        &self,
        owner: UserId,
        def: &WorkflowDef,
        expected: Option<u64>,
        keep_state: bool,
    ) -> Result<Put, StoreError> {
        let now = self.now().as_millis();
        let repos = serde_json::to_string(&def.repos)?;
        let plan = def.plan.as_ref().map(Plan::to_json).transpose()?;
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let found = existing(&tx, owner, def)?;
        let current = found.as_ref().map(|f| f.revision);
        if expected.is_some_and(|want| current != Some(want)) {
            return Ok(Put::Stale { current });
        }
        let (id, revision) = if let Some(Existing {
            id,
            enabled_at: was_enabled_at,
            ..
        }) = found
        {
            let enabled_at = if keep_state {
                was_enabled_at
            } else {
                def.enabled.then_some(was_enabled_at.unwrap_or(now))
            };
            replace_workflow(&tx, id, def, (repos, plan), (enabled_at, now))?;
            (id, current.unwrap_or_default() + 1)
        } else {
            (insert_workflow(&tx, owner, def, (repos, plan), now)?, 0)
        };
        tx.commit()?;
        drop(writer);
        Ok(Put::Written {
            id: WorkflowId::new(id),
            revision,
        })
    }
}
