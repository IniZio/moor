//! Run records: begin, finish, in-flight detection with ghost healing, and recorded-run lookups.

use moor_core::domain::decision::{Intent, IntentChange};
use moor_core::domain::event::Event;
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, RunId, Sha};
use rusqlite::types::Value;
use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};

use super::apply::{insert_event, intent_sets, update};
use super::{load_task, Store, StoreError};

/// A run to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewRun {
    /// Owning task.
    pub(crate) task: BoxId,
    /// Command line.
    pub(crate) argv: Vec<String>,
    /// Gate name when the run is recorded.
    pub(crate) record: Option<String>,
    /// Commit the run covers.
    pub(crate) sha: Option<Sha>,
    /// Sync mode used.
    pub(crate) sync: String,
    /// Process id of the runner.
    pub(crate) pid: u32,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunOutcome {
    /// Exit code, if the command exited.
    pub(crate) exit: Option<i32>,
    /// True when the run was cut short.
    pub(crate) interrupted: bool,
    /// Why the run failed before the command reported an exit.
    pub(crate) error: Option<String>,
}

impl RunOutcome {
    /// A run whose command reported `exit`, or was cut short.
    pub(crate) const fn ended(exit: Option<i32>, interrupted: bool) -> Self {
        Self {
            exit,
            interrupted,
            error: None,
        }
    }
}

/// A stored run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunRow {
    /// Run id.
    pub(crate) id: RunId,
    /// Owning task.
    pub(crate) task: BoxId,
    /// Command line.
    pub(crate) argv: Vec<String>,
    /// Gate name when the run is recorded.
    pub(crate) record: Option<String>,
    /// Commit the run covers.
    pub(crate) sha: Option<Sha>,
    /// Process id of the runner.
    pub(crate) pid: u32,
    /// Exit code, if the command exited.
    pub(crate) exit: Option<i32>,
    /// True when the run was cut short.
    pub(crate) interrupted: bool,
    /// When the run began.
    pub(crate) started_at: Ts,
    /// When the run ended; none while in flight.
    pub(crate) finished_at: Option<Ts>,
}

/// Columns read into a [`RunRow`].
const RUN_SELECT: &str =
    "SELECT id, box, argv, record, sha, pid, exit, interrupted, started_at, finished_at FROM runs";

/// Map a `runs` row selected with [`RUN_SELECT`].
fn run_from_row(row: &Row<'_>) -> Result<RunRow, StoreError> {
    let argv: String = row.get("argv")?;
    let sha: Option<String> = row.get("sha")?;
    let sha = sha
        .map(|s| Sha::parse(&s).map_err(|e| StoreError::Corrupt(format!("run sha: {e}"))))
        .transpose()?;
    Ok(RunRow {
        id: RunId::new(row.get("id")?),
        task: BoxId::new(row.get("box")?),
        argv: serde_json::from_str(&argv)?,
        record: row.get("record")?,
        sha,
        pid: row.get("pid")?,
        exit: row.get("exit")?,
        interrupted: row.get("interrupted")?,
        started_at: Ts::from_millis(row.get("started_at")?),
        finished_at: row
            .get::<_, Option<i64>>("finished_at")?
            .map(Ts::from_millis),
    })
}

impl Store {
    /// Record a run start: run row, `Running` intent and `run_started` event.
    pub(crate) fn begin_run(&self, r: NewRun) -> Result<RunId, StoreError> {
        let NewRun {
            task,
            argv,
            record,
            sha,
            sync,
            pid,
        } = r;
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if load_task(&tx, task)?.is_none() {
            return Err(StoreError::NotFound(task));
        }
        let argv = serde_json::to_string(&argv)?;
        tx.execute(
            "INSERT INTO runs (box, argv, record, sha, sync, pid, started_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                task.get(),
                argv,
                record,
                sha.as_ref().map(|s| s.as_str().to_owned()),
                sync,
                pid,
                now.as_millis()
            ],
        )?;
        let run = RunId::new(tx.last_insert_rowid());
        let intent = IntentChange::Set(Intent::Running { run, pid });
        let mut sets = intent_sets(intent, now);
        sets.push(("updated_at", Value::Integer(now.as_millis())));
        update(&tx, "boxes", "id = ?", &[task.get()], &sets)?;
        insert_event(&tx, task, &Event::RunStarted { run }, now)?;
        tx.commit()?;
        drop(conn);
        Ok(run)
    }

    /// Record a run end: finish the row, clear its intent, emit `ran`.
    ///
    /// A run that already finished is left untouched and emits nothing.
    pub(crate) fn finish_run(&self, id: RunId, out: RunOutcome) -> Result<(), StoreError> {
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task: Option<i64> = tx
            .query_row("SELECT box FROM runs WHERE id = ?", [id.get()], |r| {
                r.get(0)
            })
            .optional()?;
        let task = BoxId::new(
            task.ok_or_else(|| StoreError::Invalid(format!("unknown run {}", id.get())))?,
        );
        let changed = tx.execute(
            "UPDATE runs SET finished_at = ?, exit = ?, interrupted = ? \
             WHERE id = ? AND finished_at IS NULL",
            params![now.as_millis(), out.exit, out.interrupted, id.get()],
        )?;
        if changed == 0 {
            return Ok(());
        }
        let mut clear = intent_sets(IntentChange::Clear, now);
        clear.push(("updated_at", Value::Integer(now.as_millis())));
        update(
            &tx,
            "boxes",
            "id = ? AND intent_kind = 'running' AND intent_run = ?",
            &[task.get(), id.get()],
            &clear,
        )?;
        let ran = Event::Ran {
            run: Some(id),
            exit: out.exit,
            interrupted: out.interrupted,
            error: out.error,
        };
        insert_event(&tx, task, &ran, now)?;
        tx.commit()?;
        drop(conn);
        Ok(())
    }

    /// Record a run refused before any run row existed: one `ran` event with no run id.
    ///
    /// Writes no run row and leaves the task's intent untouched.
    pub(crate) fn refused_run(&self, task: BoxId) -> Result<(), StoreError> {
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ran = Event::Ran {
            run: None,
            exit: None,
            interrupted: false,
            error: None,
        };
        insert_event(&tx, task, &ran, now)?;
        tx.commit()?;
        drop(conn);
        Ok(())
    }

    /// The task's run still in flight, judged by `alive` on the runner pid.
    ///
    /// Open runs of the task whose runner is gone are finished as interrupted, so callers never
    /// see them. Older open runs are finished the same way even when their pid is alive: only one
    /// run can be live under the per-Box lock, so a live pid on an older row is a reused pid.
    pub(crate) fn in_flight_run(
        &self,
        task: BoxId,
        alive: &dyn Fn(u32) -> bool,
    ) -> Result<Option<RunRow>, StoreError> {
        let open = {
            let conn = self.reader()?;
            let mut stmt = conn.prepare(&format!(
                "{RUN_SELECT} WHERE box = ? AND finished_at IS NULL ORDER BY id DESC"
            ))?;
            let rows = stmt.query_and_then([task.get()], run_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut found = None;
        for run in open {
            if found.is_none() && alive(run.pid) {
                found = Some(run);
                continue;
            }
            self.finish_run(run.id, RunOutcome::ended(None, true))?;
        }
        Ok(found)
    }

    /// The task's most recent run, in flight or finished.
    pub(crate) fn last_run(&self, task: BoxId) -> Result<Option<RunRow>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "{RUN_SELECT} WHERE box = ? ORDER BY id DESC LIMIT 1"
        ))?;
        let mut rows = stmt.query_and_then([task.get()], run_from_row)?;
        rows.next().transpose()
    }

    /// Finished recorded runs of every gate name whose sha starts with `sha_prefix`, oldest first.
    pub(crate) fn recorded_runs(&self, sha_prefix: &str) -> Result<Vec<RunRow>, StoreError> {
        let prefix = sha_prefix.to_ascii_lowercase();
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "{RUN_SELECT} WHERE record IS NOT NULL AND finished_at IS NOT NULL \
             AND substr(sha, 1, length(?1)) = ?1 ORDER BY id"
        ))?;
        let rows = stmt.query_and_then(params![prefix], run_from_row)?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests;
