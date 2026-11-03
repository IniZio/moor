//! The logical schema an in-memory `SQLite` database holds, read from its pragmas.

use std::collections::BTreeMap;

use rusqlite::Connection;

use super::{plain_default, Column, ForeignKey, Index, Schema, Table, Ty};

/// The schema `sql` builds in a fresh database.
pub(super) fn schema(sql: &str) -> Schema {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(sql).unwrap();
    let names: Vec<String> = rows(
        &conn,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        |r| r.get(0),
    );
    let tables = names
        .into_iter()
        .map(|name| {
            let table = table(&conn, &name);
            (name, table)
        })
        .collect();
    let meta = rows(&conn, "SELECT k, v FROM meta ORDER BY k", |r| {
        Ok((r.get(0)?, r.get(1)?))
    });
    Schema { tables, meta }
}

/// Every row `sql` returns, mapped by `f`.
fn rows<T>(
    conn: &Connection,
    sql: &str,
    f: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Vec<T> {
    let mut stmt = conn.prepare(sql).unwrap();
    let mapped = stmt.query_map([], f).unwrap();
    mapped.collect::<Result<_, _>>().unwrap()
}

/// One table's columns, keys and indexes.
fn table(conn: &Connection, name: &str) -> Table {
    let cols: Vec<(String, String, bool, Option<String>, i64)> = rows(
        conn,
        &format!("SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info('{name}')"),
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    );
    let mut pk: Vec<(i64, String)> = cols
        .iter()
        .filter(|c| c.4 > 0)
        .map(|c| (c.4, c.0.clone()))
        .collect();
    pk.sort();
    let columns = cols
        .into_iter()
        .map(|(name, ty, not_null, default, pk)| Column {
            ty: match ty.to_ascii_lowercase().as_str() {
                "integer" => Ty::Int,
                "text" => Ty::Text,
                "blob" => Ty::Blob,
                other => unreachable!("column {name} has type {other}"),
            },
            name,
            not_null: not_null || pk > 0,
            default: default.as_deref().map(plain_default),
        })
        .collect();
    Table {
        columns,
        primary_key: pk.into_iter().map(|(_, c)| c).collect(),
        indexes: indexes(conn, name),
        foreign_keys: foreign_keys(conn, name),
    }
}

/// The table's indexes other than its primary key's.
fn indexes(conn: &Connection, table: &str) -> std::collections::BTreeSet<Index> {
    let list: Vec<(String, bool, String, bool)> = rows(
        conn,
        &format!("SELECT name, \"unique\", origin, partial FROM pragma_index_list('{table}')"),
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    );
    list.into_iter()
        .filter(|(_, _, origin, _)| origin != "pk")
        .map(|(name, unique, origin, partial)| {
            let cols = rows(
                conn,
                &format!("SELECT name FROM pragma_index_xinfo('{name}') WHERE key ORDER BY seqno"),
                |r| r.get::<_, Option<String>>(0),
            );
            Index {
                name: (origin == "c").then_some(name),
                cols: cols
                    .into_iter()
                    .map(|c| c.unwrap_or_else(|| "<expr>".to_owned()))
                    .collect(),
                unique,
                partial,
            }
        })
        .collect()
}

/// The table's foreign keys.
fn foreign_keys(conn: &Connection, table: &str) -> std::collections::BTreeSet<ForeignKey> {
    let list: Vec<(i64, String, String, String, String)> = rows(
        conn,
        &format!(
            "SELECT id, \"table\", \"from\", \"to\", on_delete \
             FROM pragma_foreign_key_list('{table}') ORDER BY id, seq"
        ),
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    );
    let mut by_id: BTreeMap<i64, ForeignKey> = BTreeMap::new();
    for (id, to_table, from, to, on_delete) in list {
        let fk = by_id.entry(id).or_insert_with(|| ForeignKey {
            cols: Vec::new(),
            table: to_table,
            to: Vec::new(),
            on_delete,
        });
        fk.cols.push(from);
        fk.to.push(to);
    }
    by_id.into_values().collect()
}
