//! Generic column-assignment helpers for the store writer.

use moor_core::domain::decision::{Field, Fields, NewBox};
use moor_schema::tables::{Boxes, Sessions};
use sea_query::{Expr, IntoIden, Nullable, UpdateStatement, Value};

use super::StoreError;
use crate::store::db::{self, DbExt as _, Tx};

/// Column assignments for one UPDATE, by column of one table.
pub(in crate::store) type Sets<C> = Vec<(C, Value)>;

/// Map a `Field` to a column value: `None` keeps, `Clear` sets the column's type of null.
fn field_value<T, V: Into<Value> + Nullable>(
    f: &Field<T>,
    conv: impl Fn(&T) -> V,
) -> Option<Value> {
    match f {
        Field::Keep => None,
        Field::Set(v) => Some(conv(v).into()),
        Field::Clear => Some(V::null()),
    }
}

/// Text field to column value.
fn text(f: &Field<String>) -> Option<Value> {
    field_value(f, Clone::clone)
}

/// Column assignments for the box fields.
pub(super) fn box_field_sets(s: &Fields) -> Sets<Boxes> {
    let all = [
        (Boxes::Issue, text(&s.issue)),
        (Boxes::Prompt, text(&s.prompt)),
        (Boxes::Base, text(&s.base)),
        (Boxes::Branch, text(&s.branch)),
        (Boxes::StateReason, text(&s.state_reason)),
        (
            Boxes::SetupFailedAt,
            field_value(&s.setup_failed_at, |t| *t),
        ),
        (
            Boxes::StepsPendingAt,
            field_value(&s.steps_pending_at, |t| *t),
        ),
        (Boxes::LaunchFailure, text(&s.launch_failure)),
    ];
    all.into_iter().filter_map(|(c, v)| Some((c, v?))).collect()
}

/// Column assignments for the primary session fields.
pub(super) fn session_field_sets(s: &Fields) -> Sets<Sessions> {
    let all = [
        (Sessions::WorkspaceId, text(&s.workspace_id)),
        (Sessions::AgentSessionId, text(&s.agent_session_id)),
    ];
    all.into_iter().filter_map(|(c, v)| Some((c, v?))).collect()
}

/// Column assignments for the box fields.
pub(super) fn driver_field_sets(s: &Fields) -> Sets<Boxes> {
    let all = [
        (
            Boxes::Provider,
            field_value(&s.box_provider, |p| p.as_str().to_owned()),
        ),
        (
            Boxes::Handle,
            field_value(&s.box_handle, |h| h.as_str().to_owned()),
        ),
        (Boxes::Workdir, text(&s.box_workdir)),
        (Boxes::Egress, text(&s.egress)),
        (Boxes::SleepReason, text(&s.box_sleep_reason)),
        (Boxes::Cpus, field_value(&s.box_cpus, |c| *c)),
        (Boxes::MemMib, field_value(&s.box_mem_mib, |m| *m)),
    ];
    all.into_iter().filter_map(|(c, v)| Some((c, v?))).collect()
}

/// Run the UPDATE `q` with `sets` assigned; nothing when `sets` is empty.
pub(in crate::store) fn update<C: IntoIden + Copy>(
    tx: &Tx<'_>,
    q: &mut UpdateStatement,
    sets: &[(C, Value)],
) -> Result<(), StoreError> {
    if sets.is_empty() {
        return Ok(());
    }
    tx.run(q.values(sets.iter().map(|(c, v)| (*c, Expr::val(v.clone())))))?;
    Ok(())
}

/// A UNIQUE failure on the open-branch index is a taken branch; other errors pass through.
pub(super) fn branch_taken(e: StoreError, branch: &Field<String>) -> StoreError {
    match (&e, branch) {
        (StoreError::Db(db), Field::Set(branch))
            if db
                .unique_key()
                .is_some_and(|k| k.is(&Boxes::Table, &[Boxes::Branch])) =>
        {
            StoreError::BranchTaken(branch.clone())
        }
        _ => e,
    }
}

/// A UNIQUE failure on the open-name index is a taken name; other errors pass through.
pub(super) fn name_taken(e: db::Error, new: &NewBox) -> StoreError {
    if e.unique_key()
        .is_some_and(|k| k.is(&Boxes::Table, &[Boxes::Name]))
    {
        StoreError::NameTaken(new.name.clone())
    } else {
        e.into()
    }
}
