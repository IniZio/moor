//! Keep each text sent to a Box in `box_inbox` until the turn it started ends, so a steer
//! outlives a Box that cannot take it now. A start's held prompt becomes the Box's first row;
//! `held_prompt` stays until a later step drops it.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{fk, index, int, serial_id, text};

/// Create `box_inbox` and copy every held prompt into it.
pub(super) fn write(s: &mut Script) {
    s.schema(
        Table::create()
            .table("box_inbox")
            .col(serial_id("id"))
            .col(int("box").not_null())
            .col(text("kind").not_null())
            .col(text("text").not_null())
            .col(text("sender").not_null())
            .col(int("created_at").not_null())
            .col(int("attempts").not_null().default(0))
            .col(int("session"))
            .col(int("sent_at"))
            .col(int("turn_before"))
            .col(int("status_before"))
            .foreign_key(&mut fk("box", "boxes", "id"))
            .foreign_key(&mut fk("session", "sessions", "id")),
    );
    s.schema(&index("box_inbox_box", "box_inbox", &["box", "id"]));
    s.raw(
        "INSERT INTO \"box_inbox\" (\"box\", \"kind\", \"text\", \"sender\", \"created_at\") \
         SELECT \"box\", 'prompt', \"held_prompt\", 'start', \"updated_at\" FROM \"box_state\" \
         WHERE \"held_prompt\" IS NOT NULL ORDER BY \"box\"",
    );
}
