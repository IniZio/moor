//! The reaction that shows a turn is running: added to the thread's first message when the
//! turn starts and removed when its line is posted. A reaction Slack will not take never
//! stops a turn.

use super::Turns;
use crate::api::ApiError;
use crate::db::TurnRow;

/// The emoji, by name, that marks a thread whose turn runs.
const WORKING: &str = "hourglass_flowing_sand";

impl Turns {
    /// Puts the working reaction on the thread's first message.
    pub(super) fn show_working(&self, row: &TurnRow) {
        let added = self
            .api
            .add_reaction(&self.bot_token, &row.channel, &row.thread_ts, WORKING);
        self.reacted(added, "add");
    }

    /// Takes the working reaction off the thread's first message.
    pub(super) fn hide_working(&self, row: &TurnRow) {
        let removed =
            self.api
                .remove_reaction(&self.bot_token, &row.channel, &row.thread_ts, WORKING);
        self.reacted(removed, "remove");
    }

    /// Logs a failed reaction call. A missing scope is logged once, other failures each time.
    fn reacted(&self, call: Result<(), ApiError>, verb: &str) {
        match call {
            Ok(()) => {}
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
    }
}
