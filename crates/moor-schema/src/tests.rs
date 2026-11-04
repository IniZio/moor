//! Both backends build the same logical schema from the one definition.
//!
//! Each side applies its rendering to a real database and reads the schema back from the
//! catalog, so a difference sea-query or a backend switch brings in shows up as a diff.

use std::collections::{BTreeMap, BTreeSet};

use moor_migrate::{render, Backend};

use crate::STEPS;

mod insertion_order;
mod postgres;
mod sqlite;

/// A column type as the store reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Ty {
    /// A 64-bit integer.
    Int,
    /// Text.
    Text,
    /// Bytes.
    Blob,
}

/// A column, in table order.
#[derive(Debug, PartialEq, Eq)]
struct Column {
    /// Its name.
    name: String,
    /// Its type.
    ty: Ty,
    /// Whether it refuses null; a primary key column always does.
    not_null: bool,
    /// Its default, without a backend's type cast.
    default: Option<String>,
}

/// An index other than the primary key's.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Index {
    /// Its name, none for one a unique constraint made.
    name: Option<String>,
    /// Its key columns, `<expr>` for an expression.
    cols: Vec<String>,
    /// Whether it is unique.
    unique: bool,
    /// Whether it has a `WHERE` clause.
    partial: bool,
}

/// A foreign key.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ForeignKey {
    /// The referencing columns.
    cols: Vec<String>,
    /// The referenced table.
    table: String,
    /// The referenced columns.
    to: Vec<String>,
    /// `CASCADE` or `NO ACTION`.
    on_delete: String,
}

/// A table's logical shape.
#[derive(Debug, Default, PartialEq, Eq)]
struct Table {
    /// Columns in order.
    columns: Vec<Column>,
    /// Primary key columns in key order.
    primary_key: Vec<String>,
    /// Indexes other than the primary key's.
    indexes: BTreeSet<Index>,
    /// Foreign keys.
    foreign_keys: BTreeSet<ForeignKey>,
}

/// Every table by name, and the rows of `meta`.
#[derive(Debug, PartialEq, Eq)]
struct Schema {
    /// Tables by name.
    tables: BTreeMap<String, Table>,
    /// `meta`'s rows as `(k, v)`.
    meta: Vec<(String, String)>,
}

/// Every step rendered for `backend`, in order.
fn sql(backend: Backend) -> String {
    STEPS.iter().map(|step| render(step, backend)).collect()
}

/// A default as both backends would write it: Postgres's `'[]'::text` is `'[]'`.
fn plain_default(raw: &str) -> String {
    raw.split_once("::")
        .map_or(raw, |(value, _)| value)
        .to_owned()
}

#[test]
fn sqlite_reads_back_every_table_with_its_keys() {
    let schema = sqlite::schema(&sql(Backend::Sqlite));
    assert_eq!(schema.tables.len(), 32, "{:?}", schema.tables.keys());
    let sessions = schema.tables.get("sessions").unwrap();
    assert_eq!(sessions.primary_key, ["id"]);
    assert!(sessions.indexes.contains(&Index {
        name: Some("sessions_host_key".to_owned()),
        cols: vec!["<expr>".to_owned(), "host_key".to_owned()],
        unique: true,
        partial: true,
    }));
}

#[test]
#[ignore = "needs Postgres at MOOR_TEST_POSTGRES_URL: make dev-db, then make unit-pg"]
fn postgres_builds_the_same_schema_as_sqlite() {
    let url = std::env::var("MOOR_TEST_POSTGRES_URL")
        .expect("MOOR_TEST_POSTGRES_URL names the Postgres to test against");
    let pg = postgres::schema(&url, &sql(Backend::Postgres));
    let lite = sqlite::schema(&sql(Backend::Sqlite));
    let names = |s: &Schema| s.tables.keys().cloned().collect::<Vec<_>>();
    assert_eq!(names(&pg), names(&lite));
    for (name, table) in &lite.tables {
        assert_eq!(pg.tables.get(name), Some(table), "table {name}");
    }
    assert_eq!(pg.meta, lite.meta);
}

#[test]
fn the_query_identifiers_name_every_table_and_column_in_order() {
    let schema = sqlite::schema(&sql(Backend::Sqlite));
    let built: BTreeMap<String, Vec<String>> = schema
        .tables
        .iter()
        .map(|(name, t)| {
            (
                name.clone(),
                t.columns.iter().map(|c| c.name.clone()).collect(),
            )
        })
        .collect();
    let named: BTreeMap<String, Vec<String>> = crate::tables::all().into_iter().collect();
    assert_eq!(named, built);
}

#[test]
fn every_table_is_listed_after_the_tables_its_foreign_keys_reference() {
    let schema = sqlite::schema(&sql(Backend::Sqlite));
    let order: Vec<String> = crate::tables::all().into_iter().map(|(t, _)| t).collect();
    for (at, name) in order.iter().enumerate() {
        for fk in &schema.tables.get(name).unwrap().foreign_keys {
            let parent = order.iter().position(|t| *t == fk.table);
            assert!(
                fk.table == *name || parent.is_some_and(|p| p < at),
                "{name} references {}, which is not listed before it",
                fk.table
            );
        }
    }
}
