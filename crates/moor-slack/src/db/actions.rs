//! Each thread's journal: one row per coordinator tool call that changes a Box, written before
//! the call acts and given its outcome after. A row with no outcome may or may not have acted.

use rusqlite::params;

use super::Db;
use crate::coordinator::context::Action;

impl Db {
    /// Writes a journal row for turn `turn` with no outcome yet; returns its sequence number.
    pub(crate) fn begin_action(
        &self,
        turn: i64,
        action: &Action,
        now: i64,
    ) -> rusqlite::Result<i64> {
        self.conn.query_row(
            "INSERT INTO slack_actions \
             (turn_id, seq, tool, box_name, requested_by, args, created_at) \
             SELECT ?1, coalesce(max(seq), 0) + 1, ?2, ?3, ?4, ?5, ?6 \
             FROM slack_actions WHERE turn_id = ?1 RETURNING seq",
            params![
                turn,
                action.tool,
                action.box_name,
                action.requested_by,
                action.args,
                now
            ],
            |r| r.get(0),
        )
    }

    /// Records what came of journal row `seq` of turn `turn`.
    pub(crate) fn end_action(
        &self,
        turn: i64,
        seq: i64,
        outcome: &str,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "UPDATE slack_actions SET outcome = ?3, done_at = ?4 \
                 WHERE turn_id = ?1 AND seq = ?2",
                params![turn, seq, outcome, now],
            )
            .map(drop)
    }

    /// The journal rows of turn `turn`, in the order they were written.
    pub(crate) fn turn_actions(&self, turn: i64) -> rusqlite::Result<Vec<Action>> {
        self.actions("a.turn_id = ?1", params![turn])
    }

    /// The journal rows of every turn of a thread, oldest first.
    pub(crate) fn thread_actions(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> rusqlite::Result<Vec<Action>> {
        self.actions(
            "u.channel = ?1 AND u.thread_ts = ?2",
            params![channel, thread_ts],
        )
    }

    /// The journal rows that `filter` selects, oldest first.
    fn actions(&self, filter: &str, args: impl rusqlite::Params) -> rusqlite::Result<Vec<Action>> {
        let sql = format!(
            "SELECT a.turn_id, a.requested_by, a.tool, a.box_name, a.args, a.outcome \
             FROM slack_actions a JOIN slack_turns u ON u.id = a.turn_id \
             WHERE {filter} ORDER BY a.turn_id, a.seq"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(args, |r| {
            Ok(Action {
                turn: r.get(0)?,
                requested_by: r.get(1)?,
                tool: r.get(2)?,
                box_name: r.get(3)?,
                args: r.get(4)?,
                outcome: r.get(5)?,
            })
        })?;
        rows.collect()
    }
}
