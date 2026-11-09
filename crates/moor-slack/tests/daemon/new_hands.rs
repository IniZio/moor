//! A Box whose machine was lost tells its thread whether its conversation came back on the new
//! one, and that nothing it was sent was sent again.

use serde_json::json;

use super::feed::thread_with_status;
use super::support::BOX_NAME;

const BOX_ID: i64 = 1;

#[test]
fn new_hands_tell_the_thread_whether_the_conversation_came_back_and_that_nothing_was_resent() {
    let (rig, controller, _bin, _post) = thread_with_status();
    controller.emit(
        BOX_ID,
        BOX_NAME,
        "session_restored",
        json!({"archive_at": "t"}),
    );
    rig.wait_post(&format!(
        "Box `{BOX_NAME}`'s conversation is back on its new machine, up to its last finished \
         turn. A message sent after that was not sent again, so reply here to send it again."
    ));
    controller.emit(
        BOX_ID,
        BOX_NAME,
        "session_lost",
        json!({"reason": "no archive"}),
    );
    rig.wait_post(&format!(
        "Box `{BOX_NAME}`'s conversation could not be brought back on its new machine, so its \
         agent started without it. A message it was working on was not sent again, so reply \
         here to send it again."
    ));
}
