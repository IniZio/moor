//! Keep what a Box's agent made outside moor, one row per Box and URL.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{fk_cascade, int, key, text};

/// Add the `box_outputs` table.
pub(super) fn write(s: &mut Script) {
    s.schema(
        Table::create()
            .table("box_outputs")
            .col(int("box").not_null())
            .col(text("url").not_null())
            .col(text("kind").not_null())
            .col(text("title").not_null())
            .col(int("first_at").not_null())
            .col(int("last_at").not_null())
            .index(key(&["box", "url"]).unique())
            .foreign_key(&mut fk_cascade("box", "boxes", "id")),
    );
}
