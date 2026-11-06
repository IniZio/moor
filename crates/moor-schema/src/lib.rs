//! moor's store schema, written once in Rust with `sea-query` and rendered for `SQLite` (VM and
//! host installs) and Postgres (the cluster).

use moor_migrate::sea_query::{
    ColumnDef, ForeignKey, ForeignKeyAction, ForeignKeyCreateStatement, Index, IndexCreateStatement,
};
use moor_migrate::{Backend, Script, Step};

mod baseline;
mod egress_token;
mod forward_port;
mod held_prompt;
mod insertion_order;
mod proxy_key;
mod proxy_state;
mod resume_memory;
mod suspend_check;
pub mod tables;
mod workflow_changes;
mod workflows;

/// The store's migrations, oldest first. Never edit an applied one; add a new step.
///
/// The baseline squashed the SQL migrations 1 to 44, so a file at 44 is adopted.
pub const STEPS: &[Step] = &[
    Step {
        name: "V45__baseline",
        write: baseline::write,
    },
    Step {
        name: "V46__insertion_order",
        write: insertion_order::write,
    },
    Step {
        name: "V47__proxy_key",
        write: proxy_key::write,
    },
    Step {
        name: "V48__workflows",
        write: workflows::write,
    },
    Step {
        name: "V49__workflow_changes",
        write: workflow_changes::write,
    },
    Step {
        name: "V50__resume_memory",
        write: resume_memory::write,
    },
    Step {
        name: "V51__proxy_state",
        write: proxy_state::write,
    },
    Step {
        name: "V52__egress_token",
        write: egress_token::write,
    },
    Step {
        name: "V53__forward_port",
        write: forward_port::write,
    },
    Step {
        name: "V54__suspend_check",
        write: suspend_check::write,
    },
    Step {
        name: "V55__held_prompt",
        write: held_prompt::write,
    },
];

/// A 64-bit integer column: `integer` in `SQLite`, `bigint` in Postgres.
fn int(name: &'static str) -> ColumnDef {
    ColumnDef::new(name).big_integer().take()
}

/// A text column.
fn text(name: &'static str) -> ColumnDef {
    ColumnDef::new(name).text().take()
}

/// A bytes column: `blob` in `SQLite`, `bytea` in Postgres.
fn blob(name: &'static str) -> ColumnDef {
    ColumnDef::new(name).blob().take()
}

/// A row id the database assigns on insert: the rowid alias in `SQLite`, an identity column in
/// Postgres.
fn row_id(s: &Script, name: &'static str) -> ColumnDef {
    let mut col = int(name);
    col.primary_key();
    if s.backend() == Backend::Postgres {
        col.auto_increment();
    }
    col
}

/// A row id that is never reused, even after its row is deleted.
fn serial_id(name: &'static str) -> ColumnDef {
    int(name).primary_key().auto_increment().take()
}

/// `col` references `table(to)`.
fn fk(col: &'static str, table: &'static str, to: &'static str) -> ForeignKeyCreateStatement {
    ForeignKey::create().from_col(col).to(table, to).take()
}

/// `col` references `table(to)`, and goes with the row it references.
fn fk_cascade(
    col: &'static str,
    table: &'static str,
    to: &'static str,
) -> ForeignKeyCreateStatement {
    fk(col, table, to)
        .on_delete(ForeignKeyAction::Cascade)
        .take()
}

/// A key over `cols`, for a table's primary key or unique constraint.
fn key(cols: &[&'static str]) -> IndexCreateStatement {
    let mut index = Index::create();
    for col in cols {
        index.col(*col);
    }
    index
}

/// A named index on `table(cols)`.
fn index(name: &'static str, table: &'static str, cols: &[&'static str]) -> IndexCreateStatement {
    key(cols).name(name).table(table).take()
}

#[cfg(test)]
mod tests;
