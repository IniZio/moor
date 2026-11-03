//! The logical schema a Postgres database holds, read from its catalog.

use postgres::{Client, NoTls};

use super::{plain_default, Column, ForeignKey, Index, Schema, Table, Ty};

/// The schema `sql` builds in a fresh Postgres schema, dropped again once read.
pub(super) fn schema(url: &str, sql: &str) -> Schema {
    let mut client = Client::connect(url, NoTls).unwrap();
    let name = format!("moor_schema_test_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {name} CASCADE; CREATE SCHEMA {name}; SET search_path TO {name}"
        ))
        .unwrap();
    let applied = client.batch_execute(sql);
    let read = applied.map(|()| read(&mut client));
    client
        .batch_execute(&format!("DROP SCHEMA {name} CASCADE"))
        .unwrap();
    read.unwrap()
}

/// Every table in the current schema, and `meta`'s rows.
fn read(client: &mut Client) -> Schema {
    let names: Vec<String> = client
        .query(
            "SELECT table_name::text FROM information_schema.tables \
             WHERE table_schema = current_schema() AND table_type = 'BASE TABLE'",
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect();
    let mut tables = std::collections::BTreeMap::new();
    for name in names {
        let table = table(client, &name);
        tables.insert(name, table);
    }
    let meta = client
        .query("SELECT k, v FROM meta ORDER BY k", &[])
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    Schema { tables, meta }
}

/// SQL for the column names of the attribute numbers `keys` of relation `rel`, in key order,
/// `<expr>` for an expression.
fn names(keys: &str, rel: &str) -> String {
    format!(
        "ARRAY(SELECT COALESCE(a.attname::text, '<expr>') \
         FROM unnest({keys}) WITH ORDINALITY AS k(num, ord) \
         LEFT JOIN pg_attribute a ON a.attrelid = {rel} AND a.attnum = k.num ORDER BY k.ord)"
    )
}

/// One table's columns, keys and indexes.
fn table(client: &mut Client, name: &str) -> Table {
    let primary_key: Vec<String> = client
        .query_opt(
            &format!(
                "SELECT {} FROM pg_constraint c \
                 WHERE c.conrelid = $1::text::regclass AND c.contype = 'p'",
                names("c.conkey", "c.conrelid")
            ),
            &[&name],
        )
        .unwrap()
        .map(|r| r.get(0))
        .unwrap_or_default();
    let columns = client
        .query(
            "SELECT column_name::text, data_type::text, is_nullable = 'NO', column_default::text \
             FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = $1 ORDER BY ordinal_position",
            &[&name],
        )
        .unwrap()
        .iter()
        .map(|r| {
            let name: String = r.get(0);
            let ty: String = r.get(1);
            Column {
                ty: match ty.as_str() {
                    "bigint" => Ty::Int,
                    "text" => Ty::Text,
                    "bytea" => Ty::Blob,
                    other => unreachable!("column {name} has type {other}"),
                },
                not_null: r.get(2),
                default: r.get::<_, Option<String>>(3).as_deref().map(plain_default),
                name,
            }
        })
        .collect();
    Table {
        columns,
        primary_key,
        indexes: indexes(client, name),
        foreign_keys: foreign_keys(client, name),
    }
}

/// The table's indexes other than its primary key's.
fn indexes(client: &mut Client, table: &str) -> std::collections::BTreeSet<Index> {
    let sql = format!(
        "SELECT i.relname::text, ix.indisunique, ix.indpred IS NOT NULL, c.contype IS NOT NULL, {} \
         FROM pg_index ix JOIN pg_class i ON i.oid = ix.indexrelid \
         LEFT JOIN pg_constraint c ON c.conindid = ix.indexrelid AND c.contype IN ('p', 'u') \
         WHERE ix.indrelid = $1::text::regclass AND NOT ix.indisprimary",
        names("ix.indkey::int2[]", "ix.indrelid")
    );
    client
        .query(&sql, &[&table])
        .unwrap()
        .iter()
        .map(|r| Index {
            name: (!r.get::<_, bool>(3)).then(|| r.get(0)),
            unique: r.get(1),
            partial: r.get(2),
            cols: r.get(4),
        })
        .collect()
}

/// The table's foreign keys.
fn foreign_keys(client: &mut Client, table: &str) -> std::collections::BTreeSet<ForeignKey> {
    let sql = format!(
        "SELECT {}, c.confrelid::regclass::text, {}, c.confdeltype::text FROM pg_constraint c \
         WHERE c.conrelid = $1::text::regclass AND c.contype = 'f'",
        names("c.conkey", "c.conrelid"),
        names("c.confkey", "c.confrelid")
    );
    client
        .query(&sql, &[&table])
        .unwrap()
        .iter()
        .map(|r| ForeignKey {
            cols: r.get(0),
            table: r.get(1),
            to: r.get(2),
            on_delete: match r.get::<_, String>(3).as_str() {
                "c" => "CASCADE".to_owned(),
                "a" => "NO ACTION".to_owned(),
                other => unreachable!("on delete action {other}"),
            },
        })
        .collect()
}
