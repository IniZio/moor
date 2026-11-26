//! moor's reading of a workflow's prompt: where it stands, and storing the plan or reading made.

use moor_core::reading::{Reading, ReadingOf};
use moor_core::workflow::{Plan, PlanDraft, PromptHash, WorkflowId};
use moor_schema::tables::Workflows;
use sea_query::{Expr, ExprTrait as _, Query};

use super::corrupt;
use crate::store::db::{self, DbExt as _, OptionalExtension as _};
use crate::store::readings::{hold_in, Stored};
use crate::store::{Store, StoreError};

/// `draft` as the JSON the column keeps.
fn json_of(draft: &PlanDraft) -> Result<String, StoreError> {
    serde_json::to_string(draft).map_err(StoreError::from)
}

/// The draft in `text`.
fn read(text: &str) -> Result<PlanDraft, StoreError> {
    serde_json::from_str(text).map_err(|e| corrupt("plan_draft", e))
}

impl Store {
    /// Sets where moor stands in making workflow `id`'s plan; none clears it.
    pub(crate) fn set_plan_draft(
        &self,
        id: WorkflowId,
        draft: Option<&PlanDraft>,
    ) -> Result<(), StoreError> {
        let text = draft.map(json_of).transpose()?;
        self.writer().run(
            Query::update()
                .table(Workflows::Table)
                .value(Workflows::PlanDraft, text)
                .and_where(Expr::col(Workflows::Id).eq(id.get())),
        )?;
        Ok(())
    }

    /// Replaces workflow `id`'s draft `was` with `now`, only while `was` is still the stored one,
    /// so a pass never undoes a newer prompt's draft. True when it was replaced.
    pub(crate) fn move_plan_draft(
        &self,
        id: WorkflowId,
        was: &PlanDraft,
        now: Option<&PlanDraft>,
    ) -> Result<bool, StoreError> {
        let text = now.map(json_of).transpose()?;
        let changed = self.writer().run(
            Query::update()
                .table(Workflows::Table)
                .value(Workflows::PlanDraft, text)
                .and_where(Expr::col(Workflows::Id).eq(id.get()))
                .and_where(Expr::col(Workflows::PlanDraft).eq(json_of(was)?)),
        )?;
        Ok(changed == 1)
    }

    /// Workflow `id`'s draft; none when moor is not making its plan.
    pub(crate) fn plan_draft(&self, id: WorkflowId) -> Result<Option<PlanDraft>, StoreError> {
        let text: Option<Option<String>> = self
            .reader()?
            .row(
                Query::select()
                    .column(Workflows::PlanDraft)
                    .from(Workflows::Table)
                    .and_where(Expr::col(Workflows::Id).eq(id.get())),
                |r| r.get(0),
            )
            .optional()?;
        text.flatten().as_deref().map(read).transpose()
    }

    /// Asks moor to read workflow `id`'s `prompt`, replacing gates a person wrote when
    /// `replace` is true. A Box already making one keeps its draft: the pass sees its prompt is
    /// old, closes it and asks again, so no Box is left behind.
    pub(crate) fn want_plan(
        &self,
        id: WorkflowId,
        prompt: &str,
        replace: bool,
    ) -> Result<(), StoreError> {
        let wanted = PlanDraft::wanted(prompt).replacing(replace);
        match self.plan_draft(id)? {
            None => self.set_plan_draft(id, Some(&wanted)),
            Some(d) if d.box_name.is_none() || d.failed.is_some() => {
                self.move_plan_draft(id, &d, Some(&wanted)).map(|_| ())
            }
            Some(_) => Ok(()),
        }
    }

    /// Every workflow with a draft, by id, with it.
    pub(crate) fn plan_drafts(&self) -> Result<Vec<(WorkflowId, PlanDraft)>, StoreError> {
        let reader = self.reader()?;
        let rows = reader.rows(
            Query::select()
                .columns([Workflows::Id, Workflows::PlanDraft])
                .from(Workflows::Table)
                .and_where(Expr::col(Workflows::PlanDraft).is_not_null())
                .order_by(Workflows::Id, sea_query::Order::Asc),
            |r| -> Result<(i64, String), db::Error> { Ok((r.get(0)?, r.get(1)?)) },
        )?;
        rows.map(|row| {
            let (id, text) = row?;
            Ok((WorkflowId::new(id), read(&text)?))
        })
        .collect()
    }

    /// Stores `plan` as workflow `id`'s and clears its draft, only while the workflow's prompt is
    /// the one it was made from. A plan clears the gates, as gates clear the plan. Its revision
    /// stays: the plan is moor's, not an edit. True when it was stored.
    pub(crate) fn store_made_plan(&self, id: WorkflowId, plan: &Plan) -> Result<bool, StoreError> {
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let prompt: Option<String> = tx
            .row(
                Query::select()
                    .column(Workflows::Prompt)
                    .from(Workflows::Table)
                    .and_where(Expr::col(Workflows::Id).eq(id.get())),
                |r| r.get(0),
            )
            .optional()?;
        if prompt.is_none_or(|p| PromptHash::of(&p) != plan.made_from) {
            drop(tx);
            drop(writer);
            return Ok(false);
        }
        tx.run(
            Query::update()
                .table(Workflows::Table)
                .value(Workflows::Plan, Some(plan.to_json()?))
                .value(Workflows::Gates, Option::<String>::None)
                .value(Workflows::PlanDraft, Option::<String>::None)
                .and_where(Expr::col(Workflows::Id).eq(id.get())),
        )?;
        tx.commit()?;
        drop(writer);
        Ok(true)
    }

    /// Keeps `reading`, which the reader of draft `was` read, as its workflow's, and clears
    /// `was`, in one transaction. False, with nothing written, when `was` is no longer the
    /// workflow's draft, the prompt is no longer the one read, or the reading is refused.
    pub(crate) fn hold_reader_read(
        &self,
        reading: &Reading,
        was: &PlanDraft,
    ) -> Result<bool, StoreError> {
        let ReadingOf::Automation(id) = reading.of;
        let since = self.now().as_millis();
        let was = json_of(was)?;
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let row: Option<(String, Option<String>)> = tx
            .row(
                Query::select()
                    .columns([Workflows::Prompt, Workflows::PlanDraft])
                    .from(Workflows::Table)
                    .and_where(Expr::col(Workflows::Id).eq(id.get()))
                    .and_where(Expr::col(Workflows::User).eq(reading.made_by.get())),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let current = row.is_some_and(|(prompt, draft)| {
            PromptHash::of(&prompt) == reading.context.source_hash && draft == Some(was)
        });
        if !current {
            drop(tx);
            drop(writer);
            return Ok(false);
        }
        let held = matches!(
            hold_in(&tx, reading, since)?,
            Stored::Written | Stored::Same
        );
        if held {
            tx.run(
                Query::update()
                    .table(Workflows::Table)
                    .value(Workflows::PlanDraft, Option::<String>::None)
                    .and_where(Expr::col(Workflows::Id).eq(id.get())),
            )?;
            tx.commit()?;
        } else {
            drop(tx);
        }
        drop(writer);
        Ok(held)
    }
}
