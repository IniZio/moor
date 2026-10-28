//! The profile email last read for each Slack user, so a busy thread does not ask Slack again.

use rusqlite::{params, OptionalExtension as _};

use super::Db;

impl Db {
    /// The email recorded for Slack user `user`, and the unix second it was read at.
    pub(crate) fn user_email(&self, user: &str) -> rusqlite::Result<Option<(String, i64)>> {
        self.conn
            .query_row(
                "SELECT email, fetched_at FROM slack_user_emails WHERE slack_user = ?1",
                params![user],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }

    /// Records `email` as read for Slack user `user` at `now`, replacing an older one.
    pub(crate) fn set_user_email(&self, user: &str, email: &str, now: i64) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "INSERT INTO slack_user_emails (slack_user, email, fetched_at) \
                 VALUES (?1, ?2, ?3) \
                 ON CONFLICT (slack_user) DO UPDATE \
                 SET email = excluded.email, fetched_at = excluded.fetched_at",
                params![user, email, now],
            )
            .map(drop)
    }
}
