//! The reaction that shows a turn is running: added to the message that started the turn when the
//! turn starts and removed when its line is posted. A reaction Slack will not take never
//! stops a turn.

use super::Turns;
use crate::api::ApiError;
use crate::db::{Db, TurnRow};

/// The emoji, by name, that marks a thread whose turn runs.
const WORKING: &str = "hourglass_flowing_sand";

impl Turns {
    /// Puts the working reaction on the message that started the turn.
    pub(super) fn show_working(&self, row: &TurnRow) {
        let added = self
            .api
            .add_reaction(&self.bot_token, &row.channel, &row.reacted_ts, WORKING);
        self.reacted(added, "add");
    }

    /// Takes the working reaction off the message that started the turn, and forgets the
    /// message once Slack took the call, so a restart removes only a reaction still there.
    pub(super) fn hide_working(&self, db: &Db, row: &TurnRow) {
        let removed =
            self.api
                .remove_reaction(&self.bot_token, &row.channel, &row.reacted_ts, WORKING);
        if self.reacted(removed, "remove") {
            self.written(db.clear_reacted(row.id));
        }
    }

    /// Removes the reaction a turn whose line is posted still has on its message, as a front
    /// that died between posting the line and removing the reaction leaves it.
    pub(super) fn clear_left_reactions(&self, db: &Db) {
        let left = db.reacting_after_post().unwrap_or_else(|e| {
            (self.log)(&format!("turns: slack.db: {e}"));
            Vec::new()
        });
        for row in left {
            self.hide_working(db, &row);
        }
    }

    /// Logs a failed reaction call. A missing scope is logged once, other failures each time.
    fn reacted(&self, call: Result<(), ApiError>, verb: &str) -> bool {
        match call {
            Ok(()) => return true,
            Err(ApiError::Slack(code)) if code == "missing_scope" => {
                if self.api.first_missing_reactions_write() {
                    (self.log)(
                        "slack: the bot token lacks reactions:write, so no thread shows a \
                         working reaction",
                    );
                }
            }
            Err(e) => (self.log)(&format!("slack: reaction {verb} failed: {e}")),
        }
        false
    }
}
