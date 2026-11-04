//! `moor store copy`: the `SQLite` store's rows into an empty Postgres database, all or nothing.
//!
//! The source is read in one snapshot and the target written in one transaction, table by table
//! in [`moor_schema::tables::all`] order, which puts every table after the ones its foreign keys
//! reference. Rows move as plain values through the db seam, so no table is spelled out here.

use std::path::Path;

use moor_schema::tables::{self, Meta};
use sea_query::{Alias, Asterisk, Expr, ExprTrait as _, Iden as _, Order, Query};

use super::db::{self, Db, DbExt as _, Target, Value};
use super::{StoreError, BUSY_TIMEOUT};

/// The most rows read from the source, and written to the target, in one statement. Postgres
/// binds at most 65535 values to a statement, so this times the widest table stays under that.
const BATCH: usize = 500;

/// The table recording which migration a store is at.
const HISTORY: &str = "refinery_schema_history";

/// One table's row count, the same in the source and the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Copied {
    /// The table.
    pub(crate) table: String,
    /// The rows it holds in both stores.
    pub(crate) rows: i64,
}

/// Copy every row of the `SQLite` store at `source` into the Postgres database `target`.
///
/// The source must be at exactly this build's latest migration, so a column it lacks or has extra
/// cannot slip by. The target is migrated to the same point, and must then hold no rows: a copy
/// onto data would mix two histories, while a migrated but empty database (a deploy step ran
/// `moor store migrate` on it, or an earlier copy rolled back) is the state a copy expects. The
/// copy commits only when every table's count matches the source's.
pub(crate) fn copy_to_postgres(source: &Path, target: &Target) -> Result<Vec<Copied>, StoreError> {
    let (mut from, latest) = db::open_sqlite_source(source, BUSY_TIMEOUT)?;
    let found = recorded_migration(&from).map_err(|_| {
        StoreError::Copy(format!(
            "{} is not a migrated moor store; run `moor store migrate` first",
            source.display()
        ))
    })?;
    if found != Some(latest) {
        return Err(StoreError::Copy(format!(
            "{} is at migration {}, not this moor's {latest}; run `moor store migrate` first",
            source.display(),
            found.map_or_else(|| "none".to_owned(), |v| v.to_string()),
        )));
    }
    let from = from.transaction()?;
    let mut opened = db::open_postgres(target)?;
    if let moor_migrate::Outcome::Ahead { found, known } = opened.schema {
        return Err(StoreError::Copy(format!(
            "the target is at migration {found}, past this moor's {known}"
        )));
    }
    let to = opened.writer.immediate()?;
    let tables = tables::all();
    for (table, _) in tables.iter().filter(|(t, _)| *t != Meta::Table.to_string()) {
        if count(&to, table)? > 0 {
            return Err(StoreError::Copy(format!(
                "the target already holds rows in {table}; copy into a database with none"
            )));
        }
    }
    to.run(Query::delete().from_table(Meta::Table))?;
    let mut copied = Vec::new();
    for (table, columns) in &tables {
        copy_table(&from, &to, table, columns)?;
        for column in columns {
            to.continue_ids(table, column)?;
        }
        let (was, now) = (count(&from, table)?, count(&to, table)?);
        if was != now {
            return Err(StoreError::Copy(format!(
                "{table} holds {was} rows in the source and {now} in the target after the copy; \
                 nothing was written"
            )));
        }
        copied.push(Copied {
            table: table.clone(),
            rows: now,
        });
    }
    to.commit()?;
    Ok(copied)
}

/// The latest migration `db` records, none when it records none.
fn recorded_migration(db: &impl Db) -> db::Result<Option<i64>> {
    db.row(
        Query::select()
            .expr(Expr::col(Alias::new("version")).max())
            .from(Alias::new(HISTORY)),
        |r| r.get(0),
    )
}

/// The rows `table` holds.
fn count(db: &impl Db, table: &str) -> db::Result<i64> {
    db.row(
        Query::select()
            .expr(Expr::col(Asterisk).count())
            .from(Alias::new(table)),
        |r| r.get(0),
    )
}

/// Copy `table`'s rows, `columns` of each. A table keyed by `id` moves in id order a batch at a
/// time, so the order rows were inserted in, which readers rely on, is kept.
fn copy_table(from: &impl Db, to: &impl Db, table: &str, columns: &[String]) -> db::Result<()> {
    let keyed = columns.first().is_some_and(|c| c == "id");
    let mut after = None;
    loop {
        let mut read = Query::select();
        read.columns(columns.iter().map(Alias::new))
            .from(Alias::new(table));
        if keyed {
            if let Some(last) = after {
                read.and_where(Expr::col(Alias::new("id")).gt(last));
            }
            read.order_by(Alias::new("id"), Order::Asc)
                .limit(u64::try_from(BATCH).unwrap_or(u64::MAX));
        }
        let rows = from
            .rows(read, |row| {
                (0..columns.len())
                    .map(|i| row.get::<_, Value>(i))
                    .collect::<db::Result<Vec<_>>>()
            })?
            .collect::<db::Result<Vec<_>>>()?;
        let Some(last) = rows.last() else {
            return Ok(());
        };
        let full = rows.len() == BATCH;
        if let Some(Value::Integer(id)) = last.first().filter(|_| keyed) {
            after = Some(*id);
        }
        for chunk in rows.chunks(BATCH) {
            let mut write = Query::insert();
            write
                .into_table(Alias::new(table))
                .columns(columns.iter().map(Alias::new));
            for row in chunk {
                write.values_panic(
                    row.iter()
                        .map(|v| Expr::val(sea_query::Value::from(v.clone()))),
                );
            }
            to.run(write)?;
        }
        if !keyed || !full {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests;
