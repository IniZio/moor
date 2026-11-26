//! What moor made of an automation's prompt: where a reading is kept, and the reading an
//! automation holds.
//!
//! A reading belongs to a workflow and to the person whose token paid, who owns the workflow
//! too: workflows are keyed by their owner and name, with a database id of their own. Every
//! lookup here carries that person, so a reading is never found through another's workflow.

use moor_core::gate::Gates;
use moor_core::reading::{Context, Read, Reading, ReadingId, ReadingOf, ReadingState};
use moor_core::user::UserId;
use moor_core::workflow::WorkflowId;
use moor_schema::tables::{Readings, Workflows};
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};

use super::db::{Db, DbExt as _, OptionalExtension as _};
use super::{Store, StoreError};

#[cfg(test)]
mod tests;

/// What a write of a read did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stored {
    /// Kept.
    Written,
    /// That reading was kept already, with this very read; nothing changed.
    Same,
    /// Refused: a reading of that id is kept already, with another read or another owner.
    Differs,
    /// Refused: the reading's person has no such workflow.
    NoWorkflow,
}

/// The store error for a stored value that does not parse.
fn corrupt(what: &str, e: impl std::fmt::Display) -> StoreError {
    StoreError::Corrupt(format!("readings {what}: {e}"))
}

/// Whether `owner` has workflow `of`.
fn owns(tx: &dyn Db, owner: UserId, of: WorkflowId) -> Result<bool, StoreError> {
    let found: Option<i64> = tx
        .row(
            Query::select()
                .column(Workflows::Id)
                .from(Workflows::Table)
                .and_where(Expr::col(Workflows::Id).eq(of.get()))
                .and_where(Expr::col(Workflows::User).eq(owner.get())),
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Keeps `reading` in `tx` unless one of its id is kept already.
fn insert_in(tx: &dyn Db, reading: &Reading, since: i64) -> Result<Stored, StoreError> {
    let ReadingState::Read(read) = &reading.state else {
        return Err(StoreError::Invalid(
            "only a reading in state read is kept".to_owned(),
        ));
    };
    let ReadingOf::Automation(of) = reading.of;
    let owner = reading.made_by;
    if !owns(tx, owner, of)? {
        return Ok(Stored::NoWorkflow);
    }
    let read = serde_json::to_string(read)?;
    let written = tx.run(
        Query::insert()
            .into_table(Readings::Table)
            .columns([
                Readings::Id,
                Readings::OfWorkflow,
                Readings::Owner,
                Readings::Parent,
                Readings::Source,
                Readings::Context,
                Readings::State,
                Readings::Since,
                Readings::Read,
            ])
            .values_panic([
                reading.id.as_str().into(),
                of.get().into(),
                owner.get().into(),
                reading.parent.as_ref().map(ToString::to_string).into(),
                reading.source.as_str().into(),
                serde_json::to_string(&reading.context)?.into(),
                "read".into(),
                since.into(),
                read.as_str().into(),
            ])
            .on_conflict(OnConflict::column(Readings::Id).do_nothing().to_owned()),
    )?;
    if written == 1 {
        return Ok(Stored::Written);
    }
    let same = kept_is(tx, reading, &read)?;
    Ok(if same { Stored::Same } else { Stored::Differs })
}

/// Whether the reading kept under `reading`'s id is `reading` with this `read`.
fn kept_is(tx: &dyn Db, reading: &Reading, read: &str) -> Result<bool, StoreError> {
    let ReadingOf::Automation(of) = reading.of;
    let kept: (Option<i64>, i64, String, Option<String>) = tx.row(
        Query::select()
            .columns([
                Readings::OfWorkflow,
                Readings::Owner,
                Readings::Source,
                Readings::Read,
            ])
            .from(Readings::Table)
            .and_where(Expr::col(Readings::Id).eq(reading.id.as_str())),
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let (of_kept, owner, source, read_kept) = kept;
    Ok(of_kept == Some(of.get())
        && owner == reading.made_by.get()
        && source == reading.source
        && read_kept.as_deref() == Some(read))
}

/// The JSON of the gates `reading` holds, the way a workflow keeps them; none when its read has
/// no gate.
fn gates_json(reading: &Reading) -> Result<Option<String>, StoreError> {
    let ReadingState::Read(read) = &reading.state else {
        return Ok(None);
    };
    let Some(doc) = read.gates() else {
        return Ok(None);
    };
    let gates =
        Gates::parse(&doc.gates, &doc.outcomes).map_err(|e| StoreError::Invalid(e.to_string()))?;
    Ok(Some(serde_json::to_string(
        &gates.read_from(&reading.source).doc(),
    )?))
}

/// Keeps `reading` in `tx` and makes its workflow hold it. A read with gates sets the
/// workflow's gates and clears its plan, as gates moor made always have. A read with no gate
/// leaves the workflow's gates and plan as they were. A refusal changes nothing.
pub(super) fn hold_in(tx: &dyn Db, reading: &Reading, since: i64) -> Result<Stored, StoreError> {
    let gates = gates_json(reading)?;
    let stored = insert_in(tx, reading, since)?;
    if matches!(stored, Stored::Written | Stored::Same) {
        let ReadingOf::Automation(of) = reading.of;
        let mut held = Query::update();
        held.table(Workflows::Table)
            .value(Workflows::Reading, reading.id.as_str())
            .and_where(Expr::col(Workflows::Id).eq(of.get()))
            .and_where(Expr::col(Workflows::User).eq(reading.made_by.get()));
        if let Some(gates) = gates {
            held.value(Workflows::Gates, gates)
                .value(Workflows::Plan, Option::<String>::None);
        }
        tx.run(&mut held)?;
    }
    Ok(stored)
}

/// A `readings` row as read, before it is parsed.
struct Kept {
    /// `of_workflow`.
    of: Option<i64>,
    /// `parent`.
    parent: Option<String>,
    /// `source`.
    source: String,
    /// `context`, as JSON.
    context: String,
    /// `state`.
    state: String,
    /// `read`, as JSON.
    read: Option<String>,
}

impl Kept {
    /// The reading of `owner` that has this row under `id`.
    fn parse(self, id: &ReadingId, owner: UserId) -> Result<Reading, StoreError> {
        let of = self
            .of
            .ok_or_else(|| corrupt("of_workflow", "a reading of no automation"))?;
        let read = match (self.state.as_str(), self.read) {
            ("read", Some(read)) => {
                serde_json::from_str::<Read>(&read).map_err(|e| corrupt("read", e))?
            }
            (state, _) => return Err(corrupt("state", format!("`{state}` is not kept yet"))),
        };
        Ok(Reading {
            id: id.clone(),
            of: ReadingOf::Automation(WorkflowId::new(of)),
            parent: self
                .parent
                .as_deref()
                .map(ReadingId::parse)
                .transpose()
                .map_err(|e| corrupt("parent", e))?,
            source: self.source,
            context: serde_json::from_str::<Context>(&self.context)
                .map_err(|e| corrupt("context", e))?,
            made_by: owner,
            state: ReadingState::Read(read),
        })
    }
}

impl Store {
    /// Keeps `reading`, which must be in state read, for the person who made it. The read is
    /// written once: the same read again changes nothing, another read under a kept id is refused.
    #[cfg_attr(not(test), expect(dead_code, reason = "the hand-in and Save call it"))]
    pub(crate) fn insert_read(&self, reading: &Reading) -> Result<Stored, StoreError> {
        let since = self.now().as_millis();
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let stored = insert_in(&tx, reading, since)?;
        tx.commit()?;
        drop(writer);
        Ok(stored)
    }

    /// Keeps `reading` and makes its workflow hold it, with the gates its read holds, in one
    /// transaction, so the reading and `workflows.gates` never differ. See [`hold_in`].
    #[cfg_attr(not(test), expect(dead_code, reason = "Save calls it"))]
    pub(crate) fn hold_read(&self, reading: &Reading) -> Result<Stored, StoreError> {
        let since = self.now().as_millis();
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let stored = hold_in(&tx, reading, since)?;
        tx.commit()?;
        drop(writer);
        Ok(stored)
    }

    /// `owner`'s reading `id`; none when there is none, or it is another person's.
    pub(crate) fn reading(
        &self,
        owner: UserId,
        id: &ReadingId,
    ) -> Result<Option<Reading>, StoreError> {
        let row: Option<Kept> = self
            .reader()?
            .row(
                Query::select()
                    .columns([
                        Readings::OfWorkflow,
                        Readings::Parent,
                        Readings::Source,
                        Readings::Context,
                        Readings::State,
                        Readings::Read,
                    ])
                    .from(Readings::Table)
                    .and_where(Expr::col(Readings::Id).eq(id.as_str()))
                    .and_where(Expr::col(Readings::Owner).eq(owner.get())),
                |r| {
                    Ok(Kept {
                        of: r.get(0)?,
                        parent: r.get(1)?,
                        source: r.get(2)?,
                        context: r.get(3)?,
                        state: r.get(4)?,
                        read: r.get(5)?,
                    })
                },
            )
            .optional()?;
        row.map(|kept| kept.parse(id, owner)).transpose()
    }

    /// The id of the reading `owner`'s workflow `id` holds; none when it holds none, or there is
    /// no such workflow of `owner`'s.
    pub(crate) fn workflow_reading(
        &self,
        owner: UserId,
        id: WorkflowId,
    ) -> Result<Option<ReadingId>, StoreError> {
        let held: Option<Option<String>> = self
            .reader()?
            .row(
                Query::select()
                    .column(Workflows::Reading)
                    .from(Workflows::Table)
                    .and_where(Expr::col(Workflows::Id).eq(id.get()))
                    .and_where(Expr::col(Workflows::User).eq(owner.get())),
                |r| r.get(0),
            )
            .optional()?;
        held.flatten()
            .as_deref()
            .map(ReadingId::parse)
            .transpose()
            .map_err(|e| StoreError::Corrupt(format!("workflows reading: {e}")))
    }
}
