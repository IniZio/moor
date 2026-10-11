use std::path::Path;

use rusqlite::Connection;
use serde_json::Value;

use super::exec::plain;
use super::registry::{db_path, open_registry};
use super::OldCli;
use crate::adapter::{Event, EventKind, Outcome};
use crate::world::World;

impl OldCli {
    pub(super) fn run_impl(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        argv: &[&str],
        opts: &crate::adapter::RunOpts,
    ) -> Outcome<crate::adapter::RunResult> {
        let mut args: Vec<String> = vec!["run".into(), name.into()];
        if opts.no_sync {
            args.push("--no-sync".into());
        }
        if opts.quiet {
            args.push("--quiet".into());
        }
        if let Some(rec) = &opts.record {
            args.push("--record".into());
            args.push(rec.clone());
        }
        args.push("--".into());
        args.extend(argv.iter().map(|s| s.to_string()));
        let ran_before = self
            .task_events(w, name)
            .iter()
            .filter(|e| e.kind == EventKind::Ran)
            .count();
        let (exit, stdout, stderr) = self.spawn_wait(w, cwd, &args, opts.interrupt);
        let ran: Vec<Event> = self
            .task_events(w, name)
            .into_iter()
            .filter(|e| e.kind == EventKind::Ran)
            .collect();
        let value = ran
            .get(ran_before..)
            .and_then(|new| new.last())
            .filter(|e| e.payload.get("error").is_none())
            .map(|e| crate::adapter::RunResult {
                exit,
                interrupted: e.payload["interrupted"].as_bool().unwrap_or(false),
                sha: e.payload["sha"].as_str().map(str::to_string),
            });
        plain(exit, stdout, stderr, value)
    }

    pub(super) fn cp_impl(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        box_path: &str,
        host_path: &Path,
    ) -> Outcome<()> {
        let args = vec![
            "cp".to_string(),
            name.to_string(),
            box_path.to_string(),
            host_path.to_string_lossy().into_owned(),
        ];
        let (exit, stdout, stderr) = self.spawn_wait(w, cwd, &args, false);
        plain(exit, stdout, stderr, (exit == 0).then_some(()))
    }

    pub(super) fn gate_impl(
        &self,
        w: &World,
        cwd: &Path,
        sha: &str,
        name: &str,
        command: Option<&str>,
    ) -> Outcome<crate::adapter::GateVerdict> {
        let mut args: Vec<String> = ["gate", "check", sha, "--name", name, "--json"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        if let Some(c) = command {
            args.push("--command".into());
            args.push(c.into());
        }
        let (exit, stdout, stderr) = self.spawn_wait(w, cwd, &args, false);
        let value =
            serde_json::from_str::<Value>(&stdout)
                .ok()
                .map(|j| crate::adapter::GateVerdict {
                    ok: j["ok"].as_bool().unwrap_or(false),
                    sha: j["sha"].as_str().map(str::to_string),
                    exit: j["exit"].as_i64().map(|n| n as i32),
                    task: j["task"].as_str().map(str::to_string),
                });
        plain(exit, stdout, stderr, value)
    }

    pub(super) fn runs_impl(&self, w: &World, name: &str) -> Vec<crate::adapter::RunRow> {
        let Some(conn) = open_registry(w) else {
            return Vec::new();
        };
        let mut stmt = conn
            .prepare(
                "SELECT task, sha, name, command, exit, interrupted FROM runs \
                 WHERE task = ?1 ORDER BY id",
            )
            .expect("prepare runs query");
        let rows = stmt
            .query_map([name], |r| {
                Ok(crate::adapter::RunRow {
                    task: r.get(0)?,
                    sha: r.get(1)?,
                    name: r.get(2)?,
                    command: r.get(3)?,
                    exit: r.get(4)?,
                    interrupted: r.get::<_, i64>(5)? != 0,
                })
            })
            .expect("query runs");
        rows.map(|r| r.expect("run row")).collect()
    }

    pub(super) fn seed_run_impl(&self, w: &World, row: &crate::adapter::RunRow) {
        let conn = Connection::open(db_path(w)).expect("open registry for seed");
        conn.execute(
            "INSERT INTO runs (task, worktree, sha, name, command, exit, interrupted, finished_at) \
             VALUES (?1, '', ?2, ?3, ?4, ?5, ?6, 1)",
            rusqlite::params![
                row.task,
                row.sha,
                row.name,
                row.command,
                row.exit,
                row.interrupted as i64
            ],
        )
        .expect("seed run row");
    }
}
