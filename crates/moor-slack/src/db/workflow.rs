//! What the workflow reporter keeps: each person's cursor into their runs, the skips already
//! told, the lines still to post, and the Boxes it adopts into their threads.

use moor_core::name::BoxId;
use rusqlite::params;

use super::{Db, SlackBox};

/// A line owed to a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outbox {
    /// Row id.
    pub(crate) id: i64,
    /// Channel id of the thread.
    pub(crate) channel: String,
    /// Timestamp of the thread's root message.
    pub(crate) thread_ts: String,
    /// The line.
    pub(crate) text: String,
    /// Posts that failed so far.
    pub(crate) attempts: i64,
}

impl Db {
    /// Starts following `owner`'s workflow runs, from the beginning when not followed already.
    pub(crate) fn follow_workflows(&self, owner: &str) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO workflow_cursor (owner, after) VALUES (?1, 0)",
                params![owner],
            )
            .map(drop)
    }

    /// The people whose workflow runs are followed.
    pub(crate) fn workflow_owners(&self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT owner FROM workflow_cursor ORDER BY owner")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect()
    }

    /// The last change number of `owner`'s workflow runs that was handled; 0 at first.
    pub(crate) fn workflow_cursor(&self, owner: &str) -> rusqlite::Result<i64> {
        let found = self.conn.query_row(
            "SELECT after FROM workflow_cursor WHERE owner = ?1",
            params![owner],
            |r| r.get(0),
        );
        match found {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
            other => other,
        }
    }

    /// Records that `owner`'s changes up to `after` are handled.
    pub(crate) fn set_workflow_cursor(&self, owner: &str, after: i64) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "INSERT INTO workflow_cursor (owner, after) VALUES (?1, ?2) \
                 ON CONFLICT (owner) DO UPDATE SET after = ?2",
                params![owner, after],
            )
            .map(drop)
    }

    /// Records that the last run of `workflow` on `repo` went the way `kind` names; true when
    /// the run before it did not.
    pub(crate) fn note_streak(
        &self,
        owner: &str,
        (workflow, repo, kind): (&str, &str, &str),
    ) -> rusqlite::Result<bool> {
        let added = self.conn.execute(
            "INSERT OR IGNORE INTO workflow_skips (owner, workflow, repo, kind) \
             VALUES (?1, ?2, ?3, ?4)",
            params![owner, workflow, repo, kind],
        )?;
        Ok(added == 1)
    }

    /// Records that `workflow` on `repo` went another way, which ends its streak.
    pub(crate) fn clear_streak(
        &self,
        owner: &str,
        workflow: &str,
        repo: &str,
    ) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "DELETE FROM workflow_skips WHERE owner = ?1 AND workflow = ?2 AND repo = ?3",
                params![owner, workflow, repo],
            )
            .map(drop)
    }

    /// Owes `text` from `owner`'s reporter to a thread.
    pub(crate) fn enqueue_line(
        &self,
        owner: &str,
        channel: &str,
        thread_ts: &str,
        text: &str,
    ) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "INSERT INTO workflow_outbox (owner, channel, thread_ts, text) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![owner, channel, thread_ts, text],
            )
            .map(drop)
    }

    /// The lines `owner`'s reporter owes, oldest first.
    pub(crate) fn owed_lines(&self, owner: &str) -> rusqlite::Result<Vec<Outbox>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, channel, thread_ts, text, attempts FROM workflow_outbox \
             WHERE owner = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![owner], |r| {
            Ok(Outbox {
                id: r.get(0)?,
                channel: r.get(1)?,
                thread_ts: r.get(2)?,
                text: r.get(3)?,
                attempts: r.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Forgets line `id`, which was posted or given up on.
    pub(crate) fn line_done(&self, id: i64) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM workflow_outbox WHERE id = ?1", params![id])
            .map(drop)
    }

    /// Counts one failed post of line `id`.
    pub(crate) fn line_failed(&self, id: i64) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "UPDATE workflow_outbox SET attempts = attempts + 1 WHERE id = ?1",
                params![id],
            )
            .map(drop)
    }

    /// Records `boxed`, which a workflow started, as bound to `box_id` in its thread at
    /// `now`, numbered after the thread's other Boxes. True when it was new; a Box the thread
    /// already has is left as it is.
    pub(crate) fn adopt(
        &self,
        boxed: &SlackBox,
        box_id: BoxId,
        branch: Option<&str>,
        now: i64,
    ) -> rusqlite::Result<bool> {
        let added = self.conn.execute(
            "INSERT OR IGNORE INTO slack_boxes \
             (box_name, channel, thread_ts, n, repo, prompt, requested_by, state, box_id, branch, created_at) \
             SELECT ?1, ?2, ?3, coalesce(max(n), 0) + 1, ?4, ?5, ?6, 'bound', ?7, ?8, ?9 \
             FROM slack_boxes WHERE channel = ?2 AND thread_ts = ?3",
            params![
                boxed.box_name.as_str(),
                boxed.channel,
                boxed.thread_ts,
                boxed.repo,
                boxed.prompt,
                boxed.requested_by,
                box_id.get(),
                branch,
                now
            ],
        )?;
        Ok(added == 1)
    }
}
