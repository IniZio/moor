//! The store above its database seam builds every statement with `sea-query` and never asks
//! which database it runs on, so `SQLite` and Postgres run the same store code.
//!
//! Every `.rs` file under `crates/moor/src` is checked, tests too, except the seam's backend
//! implementations in `store/db.rs` and `store/db/`. A file is flagged for a call to a method that
//! takes SQL text, a string literal that starts like SQL, `sea-query`'s raw SQL escapes, a
//! backend's query builder, `backend()` and `Backend`, or any use of a database driver.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, TokenStream, TokenTree};

/// How to clear a failure, printed after the violations.
pub const FIX: &str = "fix: build the statement with sea-query and run it with run, row or rows \
from store::db, using the moor_schema::tables identifiers. Work that differs by database belongs in \
a backend in store/db, and so does everything that names a database driver.";

/// The checked crate's sources, relative to the repo root.
const SRC: &str = "crates/moor/src";
/// The store, where a method named like a SQL-text one is one.
const STORE: &str = "crates/moor/src/store/";
/// The seam and its backend implementations, which may name a backend and run SQL text.
const SEAM: [&str; 2] = ["crates/moor/src/store/db.rs", "crates/moor/src/store/db/"];
/// Methods that take SQL text.
const TEXT_SQL: [&str; 7] = [
    "execute",
    "execute_batch",
    "execute_values",
    "query_values",
    "query_row",
    "prepare",
    "prepare_cached",
];
/// Names that pick or render for one database, or splice raw SQL into a `sea-query` statement.
const BACKEND_NAMES: [&str; 8] = [
    "Backend",
    "SqliteQueryBuilder",
    "PostgresQueryBuilder",
    "cust",
    "cust_with_values",
    "cust_with_expr",
    "cust_with_exprs",
    "custom_keyword",
];
/// The database drivers and the parts of them a caller would reach for.
const DRIVER_NAMES: [&str; 7] = [
    "rusqlite",
    "postgres",
    "tokio_postgres",
    "Connection",
    "TransactionBehavior",
    "ErrorCode",
    "SqliteFailure",
];
/// Words a string literal of SQL starts with.
const SQL_STARTS: [&str; 12] = [
    "SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "PRAGMA", "CREATE", "ALTER", "DROP", "BEGIN",
    "WHERE", "ORDER BY",
];

/// One flagged spot: its line and what is wrong there.
pub type Finding = (usize, String);

/// What `src` does that the store must not, in source order.
/// `in_store` also flags calls to methods named like the ones that take SQL text, which outside
/// the store mostly mean something else.
pub fn scan(src: &str, in_store: bool) -> Result<Vec<Finding>, String> {
    let tokens: TokenStream = src.parse().map_err(|e| format!("{e}"))?;
    let mut found = Vec::new();
    walk(tokens, in_store, &mut found);
    found.sort();
    Ok(found)
}

/// Collect the findings in `tokens` and every group nested in it.
fn walk(tokens: TokenStream, in_store: bool, found: &mut Vec<Finding>) {
    let tokens: Vec<TokenTree> = tokens.into_iter().collect();
    let mut i = 0;
    while let Some(t) = tokens.get(i) {
        let next = tokens.get(i + 1);
        match t {
            TokenTree::Group(g) => walk(g.stream(), in_store, found),
            TokenTree::Ident(id) if is_doc(id, next) => i += 2,
            TokenTree::Ident(id) => {
                let name = id.to_string();
                let line = id.span().start().line;
                let called = matches!(next, Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis);
                let method = i > 0
                    && matches!(tokens.get(i - 1), Some(TokenTree::Punct(p)) if p.as_char() == '.');
                if in_store && method && called && TEXT_SQL.contains(&name.as_str()) {
                    found.push((line, format!("`.{name}(..)` takes SQL text")));
                } else if method && called && name == "backend" {
                    found.push((
                        line,
                        "`.backend()` asks which database it runs on".to_owned(),
                    ));
                } else if BACKEND_NAMES.contains(&name.as_str()) {
                    found.push((line, format!("`{name}` is one database's SQL")));
                } else if DRIVER_NAMES.contains(&name.as_str()) {
                    found.push((line, format!("`{name}` is a database driver")));
                }
            }
            TokenTree::Literal(lit) => {
                if let Ok(s) = syn::parse_str::<syn::LitStr>(&lit.to_string()) {
                    let text = s.value();
                    let text = text.trim_start();
                    if SQL_STARTS.iter().any(|w| starts_with_word(text, w)) {
                        found.push((lit.span().start().line, "a string of SQL".to_owned()));
                    }
                }
            }
            TokenTree::Punct(_) => {}
        }
        i += 1;
    }
}

/// True for `doc` in `#[doc = ".."]`, the form doc comments take, whose text is prose.
fn is_doc(id: &proc_macro2::Ident, next: Option<&TokenTree>) -> bool {
    id == "doc" && matches!(next, Some(TokenTree::Punct(p)) if p.as_char() == '=')
}

/// True when `text` starts with the word `w` and more follows it, as in SQL rather than a lone
/// word like the HTTP method `DELETE`.
fn starts_with_word(text: &str, w: &str) -> bool {
    text.strip_prefix(w)
        .is_some_and(|rest| rest.starts_with(char::is_whitespace) && !rest.trim().is_empty())
}

/// Every `.rs` file under `dir`.
fn rs_files(dir: &Path, out: &mut BTreeSet<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            rs_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.insert(path);
        }
    }
    Ok(())
}

/// Check the crate under `root`; the number of files checked and one line per finding.
pub fn check(root: &Path) -> Result<(usize, Vec<String>), String> {
    let mut files = BTreeSet::new();
    let src = root.join(SRC);
    rs_files(&src, &mut files).map_err(|e| format!("{}: {e}", src.display()))?;
    let (mut bad, mut checked) = (Vec::new(), 0);
    for f in &files {
        let rel = f
            .strip_prefix(root)
            .unwrap_or(f)
            .to_string_lossy()
            .into_owned();
        if SEAM
            .iter()
            .any(|s| rel == *s || (s.ends_with('/') && rel.starts_with(s)))
        {
            continue;
        }
        let text = fs::read_to_string(f).map_err(|e| format!("{rel}: {e}"))?;
        let found = scan(&text, rel.starts_with(STORE)).map_err(|e| format!("{rel}: {e}"))?;
        checked += 1;
        bad.extend(
            found
                .into_iter()
                .map(|(line, what)| format!("store-sql: {rel}:{line}: {what}")),
        );
    }
    Ok((checked, bad))
}

#[cfg(test)]
mod tests;
