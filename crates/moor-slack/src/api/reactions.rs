//! Reactions on messages.

use std::sync::atomic::Ordering;

use moor_core::secret::Secret;
use serde_json::json;

use super::{Api, ApiError};

impl Api {
    /// Whether this is the first report of the missing `reactions:write` scope.
    pub(crate) fn first_missing_reactions_write(&self) -> bool {
        !self.reactions_reported.swap(true, Ordering::Relaxed)
    }

    /// Adds reaction `name` to the message `ts` of `channel`. A reaction already there is
    /// not an error.
    pub(crate) fn add_reaction(
        &self,
        bot_token: &Secret,
        channel: &str,
        ts: &str,
        name: &str,
    ) -> Result<(), ApiError> {
        let body = json!({"channel": channel, "timestamp": ts, "name": name});
        match self.call(bot_token, "reactions.add", &body) {
            Err(ApiError::Slack(code)) if code == "already_reacted" => Ok(()),
            other => other.map(|_| ()),
        }
    }

    /// Removes reaction `name` from the message `ts` of `channel`. A reaction that is not
    /// there is not an error.
    pub(crate) fn remove_reaction(
        &self,
        bot_token: &Secret,
        channel: &str,
        ts: &str,
        name: &str,
    ) -> Result<(), ApiError> {
        let body = json!({"channel": channel, "timestamp": ts, "name": name});
        match self.call(bot_token, "reactions.remove", &body) {
            Err(ApiError::Slack(code)) if code == "no_reaction" => Ok(()),
            other => other.map(|_| ()),
        }
    }
}
