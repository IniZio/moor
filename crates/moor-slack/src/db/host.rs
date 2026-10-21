//! Each thread's host Session on the controller: the one recorded, and the threads quiet
//! long enough for theirs to end.

use rusqlite::{params, OptionalExtension as _};

use super::Db;

impl Db {
    /// The thread's live host Session, as last recorded.
    pub(crate) fn root_session(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> rusqlite::Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT root_session FROM slack_threads WHERE channel = ?1 AND thread_ts = ?2",
                params![channel, thread_ts],
                |r| r.get(0),
            )
            .optional()
            .map(Option::flatten)
    }

    /// Records the thread's workdir and its live host Session.
    pub(crate) fn set_root_session(
        &self,
        channel: &str,
        thread_ts: &str,
        workdir: &str,
        session: i64,
    ) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "UPDATE slack_threads SET workdir = ?3, root_session = ?4 \
                 WHERE channel = ?1 AND thread_ts = ?2",
                params![channel, thread_ts, workdir, session],
            )
            .map(drop)
    }

    /// Threads with a live host Session that have been quiet since `cutoff`: no turn ended
    /// after it, no message waiting or taken by a turn that has not ended, and no Box starting
    /// or closing. A notice waits for the starter's next message, so it keeps nothing open.
    /// Each comes with its host Session.
    pub(crate) fn quiet_threads(
        &self,
        cutoff: i64,
    ) -> rusqlite::Result<Vec<(String, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.channel, t.thread_ts, t.root_session FROM slack_threads t \
             WHERE t.root_session IS NOT NULL \
             AND coalesce(t.last_turn_at, t.created_at) <= ?1 \
             AND NOT EXISTS (SELECT 1 FROM slack_inbox i WHERE i.channel = t.channel \
                 AND i.thread_ts = t.thread_ts AND i.kind = 'message' AND i.done_at IS NULL) \
             AND NOT EXISTS (SELECT 1 FROM slack_boxes b WHERE b.channel = t.channel \
                 AND b.thread_ts = t.thread_ts AND b.state IN ('starting', 'closing')) \
             ORDER BY t.channel, t.thread_ts",
        )?;
        let rows = stmt.query_map(params![cutoff], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    /// Forgets the thread's host Session `session` once it has ended; a newer one stays.
    pub(crate) fn clear_root_session(
        &self,
        channel: &str,
        thread_ts: &str,
        session: i64,
    ) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "UPDATE slack_threads SET root_session = NULL \
                 WHERE channel = ?1 AND thread_ts = ?2 AND root_session = ?3",
                params![channel, thread_ts, session],
            )
            .map(drop)
    }
}
