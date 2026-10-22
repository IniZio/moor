//! Generic column-assignment helpers for the store writer.

use moor_core::domain::decision::Field;
use moor_core::domain::decision::NewBox;
use rusqlite::types::Value;
use rusqlite::{ErrorCode, Transaction};

use super::StoreError;

/// Column assignments for one UPDATE.
pub(super) type Sets = Vec<(&'static str, Value)>;

/// Map a `Field` to a column value: `None` keeps, `Null` clears.
pub(super) fn field_value<T>(f: &Field<T>, conv: impl Fn(&T) -> Value) -> Option<Value> {
    match f {
        Field::Keep => None,
        Field::Set(v) => Some(conv(v)),
        Field::Clear => Some(Value::Null),
    }
}

/// Text field to column value.
pub(super) fn text(f: &Field<String>) -> Option<Value> {
    field_value(f, |s| Value::Text(s.clone()))
}

/// Run `UPDATE table SET .. WHERE cond` with static table, column and
/// condition text; `args` bind the `?` placeholders in `cond`.
pub(in crate::store) fn update(
    tx: &Transaction<'_>,
    table: &str,
    cond: &str,
    args: &[i64],
    sets: &Sets,
) -> Result<(), StoreError> {
    if sets.is_empty() {
        return Ok(());
    }
    let cols = sets
        .iter()
        .map(|(c, _)| format!("{c} = ?"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut vals: Vec<Value> = sets.iter().map(|(_, v)| v.clone()).collect();
    vals.extend(args.iter().map(|&a| Value::Integer(a)));
    tx.execute(
        &format!("UPDATE {table} SET {cols} WHERE {cond}"),
        rusqlite::params_from_iter(vals),
    )?;
    Ok(())
}

/// Extended result code for a UNIQUE constraint failure.
const SQLITE_CONSTRAINT_UNIQUE: i32 = 2067;

/// A UNIQUE failure on the open-branch index is a taken branch; other errors pass through.
pub(super) fn branch_taken(e: StoreError, branch: &Field<String>) -> StoreError {
    match (&e, branch) {
        (StoreError::Sqlite(rusqlite::Error::SqliteFailure(f, msg)), Field::Set(branch))
            if f.extended_code == SQLITE_CONSTRAINT_UNIQUE
                && msg.as_deref().is_some_and(|m| m.contains("boxes.branch")) =>
        {
            StoreError::BranchTaken(branch.clone())
        }
        _ => e,
    }
}

/// A UNIQUE failure on the open-name index is a taken name; other errors pass through.
pub(super) fn name_taken(e: rusqlite::Error, new: &NewBox) -> StoreError {
    match &e {
        rusqlite::Error::SqliteFailure(f, msg)
            if f.code == ErrorCode::ConstraintViolation
                && f.extended_code == SQLITE_CONSTRAINT_UNIQUE
                && msg
                    .as_deref()
                    .is_some_and(|m| m.contains("boxes.name") || m.contains("boxes_open")) =>
        {
            StoreError::NameTaken(new.name.clone())
        }
        _ => e.into(),
    }
}
