//! Make the queued prompt rows match the held prompts again before moor delivers from the inbox.
//! `V72__box_inbox` copied each held prompt, and moor kept giving held prompts until now, so a
//! copied row may hold a prompt already given, and a prompt held since has no row.
//! `held_prompt` stays until a later step drops it.

use moor_migrate::Script;

/// Requeue the held prompts: drop each prompt row never sent, then copy each held prompt.
pub(super) fn write(s: &mut Script) {
    s.raw("DELETE FROM \"box_inbox\" WHERE \"kind\" = 'prompt' AND \"launch\" IS NULL");
    s.raw(
        "INSERT INTO \"box_inbox\" (\"box\", \"kind\", \"text\", \"sender\", \"created_at\") \
         SELECT \"box\", 'prompt', \"held_prompt\", 'start', \"updated_at\" FROM \"box_state\" \
         WHERE \"held_prompt\" IS NOT NULL ORDER BY \"box\"",
    );
}
