//! What the workflow reporter keeps: each person's cursor into their runs, the skips already
//! told, the lines still to post, and the Boxes it adopts into their threads.

use moor_core::name::{BoxId, BoxName};
use rusqlite::{params, OptionalExtension};

use super::{BoxState, Db, SlackBox, SlackThread};

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
    /// The Box whose run thread the line belongs in, once it has one.
    pub(crate) box_name: Option<String>,
}

/// A run's thread that is owed: the Box is adopted into it once its first message posts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunThread {
    /// The run's Box.
    pub(crate) box_name: String,
    /// The Slack user who owns the workflow.
    pub(crate) owner: String,
    /// The workflow's name.
    pub(crate) workflow: String,
    /// The repo of the run.
    pub(crate) repo: String,
    /// Channel of the thread that added the workflow, and the new thread's.
    pub(crate) channel: String,
    /// Timestamp of the root message of the thread that added the workflow.
    pub(crate) origin_ts: String,
    /// The Box's id, once the run has a Box.
    pub(crate) box_id: Option<i64>,
    /// The Box's branch.
    pub(crate) branch: Option<String>,
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

    /// Owes `text` from `owner`'s reporter to a thread: the thread of Box `boxed` once it has
    /// one, else `channel` and `thread_ts`.
    pub(crate) fn enqueue_line(
        &self,
        owner: &str,
        (channel, thread_ts): (&str, &str),
        boxed: Option<&str>,
        text: &str,
    ) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "INSERT INTO workflow_outbox (owner, channel, thread_ts, text, box_name) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![owner, channel, thread_ts, text, boxed],
            )
            .map(drop)
    }

    /// The lines `owner`'s reporter owes, oldest first.
    pub(crate) fn owed_lines(&self, owner: &str) -> rusqlite::Result<Vec<Outbox>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, channel, thread_ts, text, attempts, box_name FROM workflow_outbox \
             WHERE owner = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![owner], |r| {
            Ok(Outbox {
                id: r.get(0)?,
                channel: r.get(1)?,
                thread_ts: r.get(2)?,
                text: r.get(3)?,
                attempts: r.get(4)?,
                box_name: r.get(5)?,
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

    /// The thread `boxed` is in, if any thread has it.
    pub(crate) fn box_thread(&self, boxed: &str) -> rusqlite::Result<Option<(String, String)>> {
        self.conn
            .query_row(
                "SELECT channel, thread_ts FROM slack_boxes WHERE box_name = ?1",
                params![boxed],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }

    /// Whether a run thread was ever owed for `boxed`, posted, pending or given up on.
    pub(crate) fn run_thread_known(&self, boxed: &str) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM workflow_run_threads WHERE box_name = ?1)",
            params![boxed],
            |r| r.get(0),
        )
    }

    /// Whether a run thread for `boxed` is owed and not given up on.
    pub(crate) fn run_thread_pending(&self, boxed: &str, give_up: i64) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM workflow_run_threads \
             WHERE box_name = ?1 AND opened = 0 AND attempts < ?2)",
            params![boxed, give_up],
            |r| r.get(0),
        )
    }

    /// Owes a run thread; one already owed for the Box is kept as it is.
    pub(crate) fn owe_run_thread(&self, t: &RunThread) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO workflow_run_threads \
                 (box_name, owner, workflow, repo, channel, origin_ts, box_id, branch) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    t.box_name,
                    t.owner,
                    t.workflow,
                    t.repo,
                    t.channel,
                    t.origin_ts,
                    t.box_id,
                    t.branch
                ],
            )
            .map(drop)
    }

    /// The run threads `owner` is owed and has not given up on, oldest first.
    pub(crate) fn owed_run_threads(
        &self,
        owner: &str,
        give_up: i64,
    ) -> rusqlite::Result<Vec<(RunThread, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT box_name, owner, workflow, repo, channel, origin_ts, box_id, branch, attempts \
             FROM workflow_run_threads WHERE owner = ?1 AND opened = 0 AND attempts < ?2 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![owner, give_up], |r| {
            let thread = RunThread {
                box_name: r.get(0)?,
                owner: r.get(1)?,
                workflow: r.get(2)?,
                repo: r.get(3)?,
                channel: r.get(4)?,
                origin_ts: r.get(5)?,
                box_id: r.get(6)?,
                branch: r.get(7)?,
            };
            Ok((thread, r.get(8)?))
        })?;
        rows.collect()
    }

    /// Counts one failed first post of the run thread for `boxed`.
    pub(crate) fn run_thread_failed(&self, boxed: &str) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "UPDATE workflow_run_threads SET attempts = attempts + 1 WHERE box_name = ?1",
                params![boxed],
            )
            .map(drop)
    }

    /// Opens the run thread whose first message is `ts`, owned by the workflow's owner, and
    /// adopts its Box into it when the Box is known. The owed row stays, marked opened, so a
    /// later change of the same run owes nothing again.
    pub(crate) fn open_run_thread(
        &self,
        t: &RunThread,
        ts: &str,
        now: i64,
    ) -> rusqlite::Result<()> {
        let thread = SlackThread {
            channel: t.channel.clone(),
            thread_ts: ts.to_owned(),
            owner: t.owner.clone(),
        };
        self.atomically(|| {
            self.insert_thread(&thread, now)?;
            self.conn.execute(
                "UPDATE workflow_run_threads SET opened = 1, thread_ts = ?2 WHERE box_name = ?1",
                params![t.box_name, ts],
            )?;
            self.adopt_run_box(&t.box_name, now)
        })
    }

    /// Records the id and branch of the Box of an owed run thread, and adopts the Box when
    /// the thread is open. Call inside a transaction.
    pub(crate) fn learn_run_box(
        &self,
        boxed: &str,
        (id, branch): (i64, Option<&str>),
        now: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE workflow_run_threads SET box_id = ?2, branch = ?3 \
             WHERE box_name = ?1 AND box_id IS NULL",
            params![boxed, id, branch],
        )?;
        self.adopt_run_box(boxed, now)
    }

    /// Whether an owed run thread for `boxed` already knows its Box id; none when no thread
    /// is owed.
    pub(crate) fn run_thread_has_box(&self, boxed: &str) -> rusqlite::Result<Option<bool>> {
        self.conn
            .query_row(
                "SELECT box_id IS NOT NULL FROM workflow_run_threads WHERE box_name = ?1",
                params![boxed],
                |r| r.get(0),
            )
            .optional()
    }

    /// The thread that was opened for the run of `boxed`, if it was.
    pub(crate) fn run_thread_at(&self, boxed: &str) -> rusqlite::Result<Option<(String, String)>> {
        self.conn
            .query_row(
                "SELECT channel, thread_ts FROM workflow_run_threads \
                 WHERE box_name = ?1 AND thread_ts IS NOT NULL",
                params![boxed],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }

    /// Adopts the Box of `boxed`'s run thread into it once both the thread and the Box id are
    /// known. The one place a run's Box is adopted.
    fn adopt_run_box(&self, boxed: &str, now: i64) -> rusqlite::Result<()> {
        let found = self
            .conn
            .query_row(
                "SELECT owner, repo, channel, thread_ts, box_id, branch FROM workflow_run_threads \
                 WHERE box_name = ?1 AND thread_ts IS NOT NULL AND box_id IS NOT NULL",
                params![boxed],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()?;
        let (Some((owner, repo, channel, thread_ts, id, branch)), Ok(box_name)) =
            (found, BoxName::parse_allow_bare(boxed))
        else {
            return Ok(());
        };
        let row = SlackBox {
            channel,
            thread_ts,
            box_name,
            repo,
            prompt: "workflow run".to_owned(),
            base: None,
            node: None,
            cold: false,
            requested_by: owner,
            parent: None,
            state: BoxState::Starting,
        };
        self.adopt(&row, BoxId::new(id), branch.as_deref(), now)
            .map(drop)
    }
}
