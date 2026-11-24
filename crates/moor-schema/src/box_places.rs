//! Let a Box be born a place, and keep each claim of a place in its own row.

use moor_migrate::sea_query::{Expr, ExprTrait, Table};
use moor_migrate::Script;

use crate::{fk, index, int, key, text};

/// Add the boxes' `kind`, a work Box until a start says otherwise, and create `place_claims`:
/// one row for each claimed place, none for an idle one.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("boxes")
            .add_column(
                text("kind")
                    .not_null()
                    .default("work")
                    .check(Expr::col("kind").is_in(["work", "place"])),
            )
            .take(),
    );
    s.schema(
        Table::create()
            .table("place_claims")
            .col(int("box").not_null())
            .col(int("user").not_null())
            .col(text("repos").not_null())
            .col(text("prompt").not_null())
            .col(int("claimed_at").not_null())
            .col(int("served_at"))
            .col(int("steered_at"))
            .primary_key(&mut key(&["box"]))
            .foreign_key(&mut fk("box", "boxes", "id"))
            .foreign_key(&mut fk("user", "users", "id")),
    );
    s.schema(&index(
        "place_claims_user",
        "place_claims",
        &["user", "claimed_at"],
    ));
}
