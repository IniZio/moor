//! The repos each channel works on, each tied to the Slack user who added it, with at most
//! one default per channel.

use rusqlite::{params, OptionalExtension};

use super::Db;

/// One repo of a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChannelRepo {
    /// The repo as the user typed it, such as `acme/api`.
    pub(crate) repo: String,
    /// Slack user id of the person who added it.
    pub(crate) added_by: String,
    /// True for the channel's default repo.
    pub(crate) is_default: bool,
}

impl Db {
    /// The repos of `channel`, sorted by name.
    pub(crate) fn channel_repos(&self, channel: &str) -> rusqlite::Result<Vec<ChannelRepo>> {
        let mut stmt = self.conn.prepare(
            "SELECT repo, added_by, is_default FROM slack_channel_repos \
             WHERE channel = ?1 ORDER BY repo",
        )?;
        let rows = stmt.query_map(params![channel], |r| {
            Ok(ChannelRepo {
                repo: r.get(0)?,
                added_by: r.get(1)?,
                is_default: r.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// Adds `repo` to `channel` for `user` at `now`; false when the channel already has it,
    /// which keeps the first adder.
    pub(crate) fn add_channel_repo(
        &self,
        channel: &str,
        repo: &str,
        user: &str,
        now: i64,
    ) -> rusqlite::Result<bool> {
        let added = self.conn.execute(
            "INSERT OR IGNORE INTO slack_channel_repos (channel, repo, added_by, added_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![channel, repo, user, now],
        )?;
        Ok(added == 1)
    }

    /// Removes `repo` from `channel`, and with it the default mark when it had one; false when
    /// the channel did not have it.
    pub(crate) fn remove_channel_repo(&self, channel: &str, repo: &str) -> rusqlite::Result<bool> {
        let removed = self.conn.execute(
            "DELETE FROM slack_channel_repos WHERE channel = ?1 AND repo = ?2",
            params![channel, repo],
        )?;
        Ok(removed == 1)
    }

    /// Marks `repo` as the default of `channel` and unmarks the previous default; false when
    /// the channel does not have `repo`. Call it inside [`Db::atomically`], so a failure
    /// cannot leave the channel without its old default.
    pub(crate) fn set_default_channel_repo(
        &self,
        channel: &str,
        repo: &str,
    ) -> rusqlite::Result<bool> {
        let known = self
            .conn
            .query_row(
                "SELECT 1 FROM slack_channel_repos WHERE channel = ?1 AND repo = ?2",
                params![channel, repo],
                |_| Ok(()),
            )
            .optional()?;
        if known.is_none() {
            return Ok(false);
        }
        self.conn.execute(
            "UPDATE slack_channel_repos SET is_default = 0 WHERE channel = ?1 AND is_default = 1",
            params![channel],
        )?;
        self.conn.execute(
            "UPDATE slack_channel_repos SET is_default = 1 WHERE channel = ?1 AND repo = ?2",
            params![channel, repo],
        )?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
