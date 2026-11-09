//! Generic column-assignment helpers for the store writer.

use moor_core::domain::decision::{Field, Fields, HandsFields, NewBox};
use moor_schema::tables::{BoxHands, BoxState, Sessions};
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

/// Column assignments for the Box state fields.
pub(super) fn box_field_sets(s: &Fields) -> Sets<BoxState> {
    let all = [
        (BoxState::Prompt, text(&s.prompt)),
        (BoxState::StateReason, text(&s.state_reason)),
        (
            BoxState::SetupFailedAt,
            field_value(&s.setup_failed_at, |t| *t),
        ),
        (
            BoxState::StepsPendingAt,
            field_value(&s.steps_pending_at, |t| *t),
        ),
        (BoxState::LaunchFailure, text(&s.launch_failure)),
    ];
    all.into_iter().filter_map(|(c, v)| Some((c, v?))).collect()
}

/// Column assignments for the primary session fields.
pub(super) fn session_field_sets(s: &Fields) -> Sets<Sessions> {
    let all = [(Sessions::AgentSessionId, text(&s.agent_session_id))];
    all.into_iter().filter_map(|(c, v)| Some((c, v?))).collect()
}

/// Column assignments for the hands fields.
pub(super) fn driver_field_sets(s: &HandsFields) -> Sets<BoxHands> {
    let all = [
        (
            BoxHands::Provider,
            field_value(&s.provider, |p| p.as_str().to_owned()),
        ),
        (
            BoxHands::Handle,
            field_value(&s.handle, |h| h.as_str().to_owned()),
        ),
        (BoxHands::Workdir, text(&s.workdir)),
        (BoxHands::Egress, text(&s.egress)),
        (BoxHands::SleepReason, text(&s.sleep_reason)),
        (BoxHands::Cpus, field_value(&s.cpus, |c| *c)),
        (BoxHands::MemMib, field_value(&s.mem_mib, |m| *m)),
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

/// A UNIQUE failure on a live Box's branch is a taken branch; other errors pass through.
pub(super) fn branch_taken(e: StoreError, branch: Option<&String>) -> StoreError {
    match (&e, branch) {
        (StoreError::Db(db), Some(branch))
            if db
                .unique_key()
                .is_some_and(|k| k.is(&BoxState::Table, &[BoxState::OpenBranch])) =>
        {
            StoreError::BranchTaken(branch.clone())
        }
        _ => e,
    }
}

/// A UNIQUE failure on a live Box's name is a taken name; other errors pass through.
pub(super) fn name_taken(e: db::Error, new: &NewBox) -> StoreError {
    if e.unique_key()
        .is_some_and(|k| k.is(&BoxState::Table, &[BoxState::OpenName]))
    {
        StoreError::NameTaken(new.name.clone())
    } else {
        e.into()
    }
}
