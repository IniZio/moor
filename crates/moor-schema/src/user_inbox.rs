//! Keep what needed each person: an agent's question or permission, a workflow run that waits at
//! an approve step, and a run that failed. A row is written with its event, carries what the
//! inbox shows, and holds the person's read mark.

use moor_migrate::sea_query::{Expr, ExprTrait, Table};
use moor_migrate::Script;

use crate::{fk, fk_cascade, index, int, key, serial_id, text};

/// Create `user_inbox`.
pub(super) fn write(s: &mut Script) {
    s.schema(
        Table::create()
            .table("user_inbox")
            .col(serial_id("id"))
            .col(int("user").not_null())
            .col(int("box").not_null())
            .col(int("event").not_null())
            .col(text("kind").not_null().check(Expr::col("kind").is_in([
                "needs_input",
                "run_waiting",
                "run_failed",
            ])))
            .col(text("title").not_null())
            .col(text("text").not_null())
            .col(int("created_at").not_null())
            .col(int("read_at"))
            .index(key(&["event"]).unique())
            .foreign_key(&mut fk_cascade("user", "users", "id"))
            .foreign_key(&mut fk("box", "boxes", "id")),
    );
    s.schema(&index("user_inbox_user", "user_inbox", &["user", "id"]));
}
