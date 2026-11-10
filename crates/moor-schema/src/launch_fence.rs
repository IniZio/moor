//! Fence agent work by launch: a write made from a reading of one launch lands only while that
//! launch is its Box's live one, the newest not ended. Every fenced write reads that.

use moor_migrate::Script;

use crate::index;

/// Index launches by Box, newest last. End each launch an older moor left live behind a newer
/// one, at the start of the launch after it, and each launch begun before the Box's newest
/// hands, when those hands were made: its agent ran on hands the Box no longer has.
pub(super) fn write(s: &mut Script) {
    s.schema(&index("launches_box", "launches", &["box", "id"]));
    s.raw(
        "UPDATE \"launches\" SET \"ended_at\" = (SELECT MIN(\"l\".\"started_at\") \
         FROM \"launches\" AS \"l\" WHERE \"l\".\"box\" = \"launches\".\"box\" \
         AND \"l\".\"id\" > \"launches\".\"id\") \
         WHERE \"ended_at\" IS NULL AND EXISTS (SELECT 1 FROM \"launches\" AS \"l\" \
         WHERE \"l\".\"box\" = \"launches\".\"box\" AND \"l\".\"id\" > \"launches\".\"id\")",
    );
    let newest_hands = "(SELECT MAX(\"h\".\"created_at\") FROM \"box_hands\" AS \"h\" \
                        WHERE \"h\".\"box\" = \"launches\".\"box\")";
    s.raw(&format!(
        "UPDATE \"launches\" SET \"ended_at\" = {newest_hands} \
         WHERE \"ended_at\" IS NULL AND \"started_at\" < {newest_hands}"
    ));
}
