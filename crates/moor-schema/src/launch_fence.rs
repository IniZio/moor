//! Fence agent work by launch: a write made from a reading of one launch lands only while that
//! launch is its Box's live one, the newest not ended. Every fenced write reads that.

use moor_migrate::Script;

use crate::index;

/// Index launches by Box, newest last, and end each launch an older moor left live behind a
/// newer one, at the start of the launch after it.
pub(super) fn write(s: &mut Script) {
    s.schema(&index("launches_box", "launches", &["box", "id"]));
    s.raw(
        "UPDATE \"launches\" SET \"ended_at\" = (SELECT MIN(\"l\".\"started_at\") \
         FROM \"launches\" AS \"l\" WHERE \"l\".\"box\" = \"launches\".\"box\" \
         AND \"l\".\"id\" > \"launches\".\"id\") \
         WHERE \"ended_at\" IS NULL AND EXISTS (SELECT 1 FROM \"launches\" AS \"l\" \
         WHERE \"l\".\"box\" = \"launches\".\"box\" AND \"l\".\"id\" > \"launches\".\"id\")",
    );
}
