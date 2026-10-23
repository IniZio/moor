//! The coordinator's tool server as one turn starts it.

use moor_core::user::Identity;

use super::Turns;
use crate::db::TurnRow;

impl Turns {
    /// The tool server's arguments for the thread of `row`, which it speaks for as
    /// `identity`; they name no secret.
    pub(super) fn tool_args(&self, row: &TurnRow, identity: &Identity) -> Vec<String> {
        let state_dir =
            std::path::absolute(&self.state_dir).unwrap_or_else(|_| self.state_dir.clone());
        vec![
            "mcp".to_owned(),
            "--state-dir".to_owned(),
            state_dir.display().to_string(),
            "--socket".to_owned(),
            self.socket.display().to_string(),
            "--thread".to_owned(),
            format!("{}/{}", row.channel, row.thread_ts),
            "--max-open-boxes".to_owned(),
            self.max_open_boxes.to_string(),
            "--as".to_owned(),
            identity.to_string(),
        ]
    }
}
