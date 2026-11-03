//! Migrations written once in Rust with `sea-query` and rendered for each backend.

use refinery_core::Migration;
use sea_query::{
    PostgresQueryBuilder, QueryStatementWriter, SchemaStatementBuilder, SqliteQueryBuilder,
};

use crate::Error;

/// The database a migration is rendered for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// `SQLite`, for VM and host installs.
    Sqlite,
    /// Postgres, for the cluster.
    Postgres,
}

/// One migration: its refinery name (`V<n>__<name>`) and the statements it writes.
#[derive(Debug, Clone, Copy)]
pub struct Step {
    /// `V<n>__<name>`, as a migration file would be named.
    pub name: &'static str,
    /// Adds this migration's statements to a script for one backend.
    pub write: fn(&mut Script),
}

/// The SQL of one migration for one backend, built statement by statement.
#[derive(Debug)]
pub struct Script {
    /// The backend every statement is rendered for.
    backend: Backend,
    /// The statements so far, each ended with `;`.
    sql: String,
}

impl Script {
    /// The backend this script renders for, for the rare statement that differs between them.
    #[must_use]
    pub const fn backend(&self) -> Backend {
        self.backend
    }

    /// Add a schema statement: a table, index or foreign key change.
    pub fn schema(&mut self, stmt: &impl SchemaStatementBuilder) -> &mut Self {
        let sql = match self.backend {
            Backend::Sqlite => stmt.to_string(SqliteQueryBuilder),
            Backend::Postgres => stmt.to_string(PostgresQueryBuilder),
        };
        self.push(&sql)
    }

    /// Add a data statement, with its values written inline.
    pub fn query(&mut self, stmt: &impl QueryStatementWriter) -> &mut Self {
        let sql = match self.backend {
            Backend::Sqlite => stmt.to_string(SqliteQueryBuilder),
            Backend::Postgres => stmt.to_string(PostgresQueryBuilder),
        };
        self.push(&sql)
    }

    /// Add a statement written as SQL, for one `sea-query` cannot render for this backend.
    pub fn raw(&mut self, sql: &str) -> &mut Self {
        self.push(sql)
    }

    /// Append one statement.
    fn push(&mut self, sql: &str) -> &mut Self {
        self.sql.push_str(sql);
        self.sql.push_str(";\n");
        self
    }
}

/// The SQL `step` renders for `backend`.
#[must_use]
pub fn render(step: &Step, backend: Backend) -> String {
    let mut script = Script {
        backend,
        sql: String::new(),
    };
    (step.write)(&mut script);
    script.sql
}

/// Build the migration list for `backend` from steps in version order.
pub fn steps(set: &[Step], backend: Backend) -> Result<Vec<Migration>, Error> {
    Ok(set
        .iter()
        .map(|step| Migration::unapplied(step.name, &render(step, backend)))
        .collect::<Result<_, _>>()?)
}
