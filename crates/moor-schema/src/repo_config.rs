//! Keep each repo's config in the store, edited there.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{int, text};

/// Add the `repos` table: one row per repo, keyed by its lowercased `owner/name`. `config` is the
/// YAML text the repo config parser reads, `revision` counts its writes from 1, and `edited_by`
/// names who wrote it last, with no key to `users` so a deleted user leaves the config.
pub(super) fn write(s: &mut Script) {
    s.schema(
        Table::create()
            .table("repos")
            .col(text("repo").primary_key())
            .col(text("config").not_null())
            .col(int("revision").not_null())
            .col(text("edited_by").not_null())
            .col(int("edited_at").not_null()),
    );
}
