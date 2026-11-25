//! Keep each repo's Proxy rules in the store, edited there.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{int, text};

/// Add the `repo_rules` table: one row per repo, keyed by its lowercased `owner/name`, as
/// `repos` is. `rules` is the rules text the Proxy follows for the repo's Boxes, `revision`
/// counts its writes from 1, and `edited_by` names who wrote it last. A repo with no row follows
/// the default rules text.
pub(super) fn write(s: &mut Script) {
    s.schema(
        Table::create()
            .table("repo_rules")
            .col(text("repo").primary_key())
            .col(text("rules").not_null())
            .col(int("revision").not_null())
            .col(text("edited_by").not_null())
            .col(int("edited_at").not_null()),
    );
}
