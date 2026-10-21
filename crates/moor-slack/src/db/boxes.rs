//! The Boxes each thread asked for, one row per Box, each tied to the Slack user who asked.

use moor_core::name::{BoxId, BoxName, SessionId};
use rusqlite::types::Type;
use rusqlite::{params, Row};

use super::Db;

/// Columns of a Box row, in the order [`box_row`] reads them.
const BOX_COLUMNS: &str = "b.channel, b.thread_ts, b.box_name, b.repo, b.goal, b.requested_by, \
     b.state, b.box_id, b.branch, b.parent FROM slack_boxes b";

/// Where a Box stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoxState {
    /// The start was sent or is about to be; no reply has bound it yet.
    Starting,
    /// The controller named this Box.
    Bound {
        /// The Box's id.
        box_id: BoxId,
        /// The Box's branch, when the reply named one.
        branch: Option<String>,
    },
    /// Its requester asked to close it; the close is sent or about to be.
    Closing {
        /// The Box's id.
        box_id: BoxId,
        /// The Box's branch, when the start reply named one.
        branch: Option<String>,
    },
    /// The controller closed it.
    Gone {
        /// The Box's id.
        box_id: BoxId,
        /// The Box's branch, when the start reply named one.
        branch: Option<String>,
    },
}

/// A Box a thread asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlackBox {
    /// Channel id of the thread.
    pub(crate) channel: String,
    /// Timestamp of the thread's root message.
    pub(crate) thread_ts: String,
    /// Name of the Box.
    pub(crate) box_name: BoxName,
    /// Repo the Box works on.
    pub(crate) repo: String,
    /// What the Box should do.
    pub(crate) goal: String,
    /// Slack user id of the person who asked for the Box.
    pub(crate) requested_by: String,
    /// The host Session of the thread when the Box was asked for, which becomes its parent.
    pub(crate) parent: Option<SessionId>,
    /// Where the Box stands.
    pub(crate) state: BoxState,
}

impl Db {
    /// The Boxes of a thread, in the order the thread asked for them.
    pub(crate) fn thread_boxes(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> rusqlite::Result<Vec<SlackBox>> {
        let sql =
            format!("SELECT {BOX_COLUMNS} WHERE b.channel = ?1 AND b.thread_ts = ?2 ORDER BY b.n");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![channel, thread_ts], box_row)?;
        rows.collect()
    }

    /// Boxes still in `starting`, oldest first.
    pub(crate) fn starting(&self) -> rusqlite::Result<Vec<SlackBox>> {
        let sql = format!(
            "SELECT {BOX_COLUMNS} WHERE b.state = 'starting' \
             ORDER BY b.created_at, b.channel, b.thread_ts, b.n"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], box_row)?;
        rows.collect()
    }

    /// Boxes still in `closing`, oldest first.
    pub(crate) fn closing(&self) -> rusqlite::Result<Vec<SlackBox>> {
        let sql = format!(
            "SELECT {BOX_COLUMNS} WHERE b.state = 'closing' \
             ORDER BY b.created_at, b.channel, b.thread_ts, b.n"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], box_row)?;
        rows.collect()
    }

    /// Records `boxed` in `starting` at `now`, numbered after its thread's other Boxes. Its
    /// thread must have a row; a name already taken is an error.
    pub(crate) fn insert_starting(&self, boxed: &SlackBox, now: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO slack_boxes \
             (box_name, channel, thread_ts, n, repo, goal, requested_by, parent, state, created_at) \
             SELECT ?1, ?2, ?3, coalesce(max(n), 0) + 1, ?4, ?5, ?6, ?7, 'starting', ?8 \
             FROM slack_boxes WHERE channel = ?2 AND thread_ts = ?3",
            params![
                boxed.box_name.as_str(),
                boxed.channel,
                boxed.thread_ts,
                boxed.repo,
                boxed.goal,
                boxed.requested_by,
                boxed.parent.map(SessionId::get),
                now
            ],
        )?;
        Ok(())
    }

    /// Binds a `starting` Box to the id the controller gave it.
    pub(crate) fn bind(
        &self,
        name: &BoxName,
        box_id: BoxId,
        branch: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE slack_boxes SET state = 'bound', box_id = ?2, branch = ?3 \
             WHERE box_name = ?1 AND state = 'starting'",
            params![name.as_str(), box_id.get(), branch],
        )?;
        Ok(())
    }

    /// Marks a `bound` Box `closing`.
    pub(crate) fn mark_closing(&self, name: &BoxName) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE slack_boxes SET state = 'closing' WHERE box_name = ?1 AND state = 'bound'",
            params![name.as_str()],
        )?;
        Ok(())
    }

    /// Ends the close of a `closing` Box: `gone` when the controller closed it, else `bound`
    /// again.
    pub(crate) fn settle_close(&self, name: &BoxName, gone: bool) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "UPDATE slack_boxes SET state = ?2 WHERE box_name = ?1 AND state = 'closing'",
                params![name.as_str(), if gone { "gone" } else { "bound" }],
            )
            .map(drop)
    }

    /// Marks a `bound` Box `gone`, once the controller says it has no such Box.
    pub(crate) fn mark_gone(&self, name: &BoxName) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "UPDATE slack_boxes SET state = 'gone' WHERE box_name = ?1 AND state = 'bound'",
                params![name.as_str()],
            )
            .map(drop)
    }

    /// Deletes a Box still in `starting`, so its thread can ask for it again. The thread stays.
    pub(crate) fn forget_starting(&self, name: &BoxName) -> rusqlite::Result<()> {
        self.conn
            .execute(
                "DELETE FROM slack_boxes WHERE box_name = ?1 AND state = 'starting'",
                params![name.as_str()],
            )
            .map(drop)
    }
}

/// Reads one Box row selected with [`BOX_COLUMNS`].
fn box_row(r: &Row<'_>) -> rusqlite::Result<SlackBox> {
    let bad = |col: usize, why: String| {
        rusqlite::Error::FromSqlConversionFailure(col, Type::Text, why.into())
    };
    let state: String = r.get(6)?;
    let (box_id, branch) = (r.get::<_, Option<i64>>(7)?.map(BoxId::new), r.get(8)?);
    let state = match (state.as_str(), box_id) {
        ("starting", _) => BoxState::Starting,
        ("bound", Some(box_id)) => BoxState::Bound { box_id, branch },
        ("closing", Some(box_id)) => BoxState::Closing { box_id, branch },
        ("gone", Some(box_id)) => BoxState::Gone { box_id, branch },
        (other, _) => return Err(bad(6, format!("Box state `{other}`"))),
    };
    let name: String = r.get(2)?;
    Ok(SlackBox {
        channel: r.get(0)?,
        thread_ts: r.get(1)?,
        box_name: BoxName::parse_allow_bare(&name).map_err(|e| bad(2, e.to_string()))?,
        repo: r.get(3)?,
        goal: r.get(4)?,
        requested_by: r.get(5)?,
        parent: r.get::<_, Option<i64>>(9)?.map(SessionId::new),
        state,
    })
}

#[cfg(test)]
mod tests;
