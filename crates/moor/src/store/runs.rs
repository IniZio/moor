//! Run records: `Store::begin_run` and `Store::finish_run`.

use moor_core::domain::decision::{Intent, IntentChange};
use moor_core::domain::event::Event;
use moor_core::{RunId, Sha, TaskId};
use rusqlite::types::Value;
use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::apply::{insert_event, intent_sets, update};
use super::{load_task, Store, StoreError};

/// A run to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewRun {
    /// Owning task.
    pub(crate) task: TaskId,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunOutcome {
    /// Exit code, if the command exited.
    pub(crate) exit: Option<i32>,
    /// True when the run was cut short.
    pub(crate) interrupted: bool,
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
            "INSERT INTO runs (task, argv, record, sha, sync, pid, started_at) \
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
        update(&tx, "tasks", "id", task.get(), &sets)?;
        insert_event(&tx, task, &Event::RunStarted { run }, now)?;
        tx.commit()?;
        drop(conn);
        Ok(run)
    }

    /// Record a run end: finish the row, clear its intent, emit `ran`.
    pub(crate) fn finish_run(&self, id: RunId, out: RunOutcome) -> Result<(), StoreError> {
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task: Option<i64> = tx
            .query_row("SELECT task FROM runs WHERE id = ?", [id.get()], |r| {
                r.get(0)
            })
            .optional()?;
        let task = TaskId::new(
            task.ok_or_else(|| StoreError::Invalid(format!("unknown run {}", id.get())))?,
        );
        tx.execute(
            "UPDATE runs SET finished_at = ?, exit = ?, interrupted = ? WHERE id = ?",
            params![now.as_millis(), out.exit, out.interrupted, id.get()],
        )?;
        let clear = intent_sets(IntentChange::Clear, now);
        let cols = clear
            .iter()
            .map(|(c, _)| format!("{c} = ?"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE tasks SET {cols}, updated_at = ? WHERE id = ? AND intent_kind = 'running' AND intent_run = ?"
        );
        let mut args: Vec<Value> = clear.into_iter().map(|(_, v)| v).collect();
        args.extend([
            Value::Integer(now.as_millis()),
            Value::Integer(task.get()),
            Value::Integer(id.get()),
        ]);
        tx.execute(&sql, rusqlite::params_from_iter(args))?;
        let ran = Event::Ran {
            run: id,
            exit: out.exit,
            interrupted: out.interrupted,
        };
        insert_event(&tx, task, &ran, now)?;
        tx.commit()?;
        drop(conn);
        Ok(())
    }
}
