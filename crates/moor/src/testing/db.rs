//! Statements tests outside the store run on its db file, to set up or read a row no store
//! method exposes.

use sea_query::{Expr, ExprTrait as _, IntoColumnRef, IntoTableRef, Query, SelectStatement};

/// `col` of the `table` rows whose `key` is `value`.
pub(crate) fn column_where(
    table: impl IntoTableRef,
    col: impl IntoColumnRef,
    key: impl IntoColumnRef,
    value: impl Into<Expr>,
) -> SelectStatement {
    Query::select()
        .column(col)
        .from(table)
        .and_where(Expr::col(key).eq(value))
        .take()
}
