use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use rusqlite::Connection;
use serde_json::Value;

use super::{
    AgentView, CloseView, EnvOpts, EnvReport, Event, EventKind, ExitClass, Moor, OpenView, Outcome,
    PaneView, RemoveView, Session, StartOpts, TaskView, WaitView,
};
use crate::world::World;

mod args;
mod env;
mod exec;
mod location;
#[path = "../old_reports.rs"]
mod old_reports;
mod registry;
mod runs;

pub use env::seam_env;

use args::{start_args, str_list};
use exec::name_args;
use registry::{db_path, open_registry, shift_hook_cache, shift_ts};

pub struct OldCli {
    pub bin: PathBuf,
}

impl Moor for OldCli {
    fn name(&self) -> &'static str {
        "old"
    }

    fn start(&self, w: &World, cwd: &Path, name: &str, opts: &StartOpts) -> Outcome<TaskView> {
        let pin = location::pin_start(w, cwd, name, opts);
        let args = start_args(w, name, opts);
        let mut o = self.run(w, cwd, &args, |j| {
            TaskView::from_json(j.get("task").unwrap_or(&Value::Null))
        });
        pin.settle_worktree(w, name);
        drop(pin);
        if o.class == ExitClass::Ok && o.value.as_ref().is_some_and(|t| t.name.is_empty()) {
            o.value = None;
        }
        o
    }

    fn list(&self, w: &World, cwd: &Path) -> Outcome<Vec<TaskView>> {
        let args = vec!["list".to_string(), "--json".to_string()];
        self.run(w, cwd, &args, |j| {
            j.as_array()
                .map(|a| a.iter().map(TaskView::from_json).collect())
                .unwrap_or_default()
        })
    }

    fn close(&self, w: &World, cwd: &Path, name: &str, _force: bool) -> Outcome<CloseView> {
        let args = vec!["close".to_string(), name.to_string(), "--json".to_string()];
        self.run(w, cwd, &args, |j| CloseView {
            task: TaskView::from_json(j),
            box_deleted: j
                .get("box_deleted")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            workspace_closed: j
                .get("workspace_closed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            sessions_stopped: str_list(j, "sessions_stopped"),
        })
    }

    fn events(&self, w: &World, name: &str) -> Vec<Event> {
        let Some(conn) = open_registry(w) else {
            return Vec::new();
        };
        let mut stmt = conn
            .prepare(
                "SELECT e.kind, e.payload FROM events e JOIN tasks t ON t.id = e.task_id \
                 WHERE t.name = ?1 ORDER BY e.id",
            )
            .expect("prepare events query");
        let rows = stmt
            .query_map([name], |r| {
                let kind: String = r.get(0)?;
                let payload: String = r.get(1)?;
                Ok(Event {
                    kind: EventKind::from_old(&kind),
                    payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                })
            })
            .expect("query events");
        rows.map(|r| r.expect("event row")).collect()
    }

    fn env(&self, w: &World, cwd: &Path, opts: &EnvOpts) -> Outcome<EnvReport> {
        let mut args = vec!["env".to_string()];
        if !opts.text {
            args.push("--json".into());
        }
        let _pin = location::pin_env(cwd);
        args.extend(["--agent-location".to_string(), "box".to_string()]);
        self.run_env(w, cwd, &args, opts, EnvReport::from_json)
    }

    fn run(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        argv: &[&str],
        opts: &super::RunOpts,
    ) -> Outcome<super::RunResult> {
        self.run_impl(w, cwd, name, argv, opts)
    }

    fn cp(&self, w: &World, cwd: &Path, name: &str, box_path: &str, host: &Path) -> Outcome<()> {
        self.cp_impl(w, cwd, name, box_path, host)
    }

    fn gate_check(
        &self,
        w: &World,
        cwd: &Path,
        sha: &str,
        name: &str,
        command: Option<&str>,
    ) -> Outcome<super::GateVerdict> {
        self.gate_impl(w, cwd, sha, name, command)
    }

    fn runs(&self, w: &World, name: &str) -> Vec<super::RunRow> {
        self.runs_impl(w, name)
    }

    fn seed_run(&self, w: &World, row: &super::RunRow) {
        self.seed_run_impl(w, row)
    }
    fn remove(&self, w: &World, cwd: &Path, name: &str, discard: bool) -> Outcome<RemoveView> {
        let mut args = vec!["remove".to_string(), name.to_string(), "--json".to_string()];
        if discard {
            args.push("--discard".into());
        }
        let flag = |j: &Value, k: &str| j.get(k).and_then(Value::as_bool).unwrap_or(false);
        self.run(w, cwd, &args, |j| RemoveView {
            task: TaskView::from_json(j),
            worktree_removed: flag(j, "worktree_removed"),
            branch_deleted: flag(j, "branch_deleted"),
            discarded: flag(j, "discarded"),
        })
    }

    fn sessions(&self, w: &World, name: &str) -> Vec<Session> {
        let Some(conn) = open_registry(w) else {
            return Vec::new();
        };
        let mut stmt = conn
            .prepare(
                "SELECT s.engine, s.session_id, s.state FROM sessions s \
                 JOIN tasks t ON t.id = s.task_id WHERE t.name = ?1 ORDER BY s.id",
            )
            .expect("prepare sessions query");
        let rows = stmt
            .query_map([name], |r| {
                let engine: String = r.get(0)?;
                let state: String = r.get(2)?;
                Ok(Session {
                    box_pane: engine == "box-pane",
                    session_id: r.get(1)?,
                    live: state == "live",
                })
            })
            .expect("query sessions");
        rows.map(|r| r.expect("session row")).collect()
    }
    fn start_detached(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        opts: &StartOpts,
    ) -> std::process::Child {
        let pin = location::pin_start(w, cwd, name, opts);
        let mut child = w
            .command(&self.bin)
            .envs(seam_env(w))
            .current_dir(cwd)
            .args(start_args(w, name, opts))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", self.bin.display()));
        w.track_pid(child.id());
        while !matches!(child.try_wait(), Ok(Some(_))) && !location::registered(w, name) {
            std::thread::sleep(Duration::from_millis(50));
        }
        pin.settle_worktree(w, name);
        drop(pin);
        child
    }

    fn wait(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        timeout_secs: Option<u64>,
    ) -> Outcome<WaitView> {
        let mut args = name_args("wait", name);
        if let Some(t) = timeout_secs {
            args.extend(["--timeout".to_string(), t.to_string()]);
        }
        self.run(w, cwd, &args, |j| WaitView {
            timed_out: j.get("timed_out").and_then(Value::as_bool).unwrap_or(false),
            run: j.get("run").filter(|r| !r.is_null()).cloned(),
            agent: j.get("agent").and_then(AgentView::from_json),
        })
    }

    fn steer(&self, w: &World, cwd: &Path, name: &str, message: &str) -> Outcome<()> {
        let mut args = name_args("steer", name);
        args.insert(2, message.to_string());
        self.run_unit(w, cwd, &args)
    }

    fn open(&self, w: &World, cwd: &Path, name: &str) -> Outcome<OpenView> {
        self.run(w, cwd, &name_args("open", name), |j| OpenView {
            task: TaskView::from_json(j),
            woke: j.get("woke").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    fn shell(&self, w: &World, cwd: &Path, name: &str) -> Outcome<()> {
        let args = vec!["shell".to_string(), name.to_string()];
        self.run_unit(w, cwd, &args)
    }

    fn pane(&self, w: &World, cwd: &Path, name: &str) -> Outcome<PaneView> {
        self.run(w, cwd, &name_args("pane", name), |j| PaneView {
            task: TaskView::from_json(j),
            agent: j.get("agent").and_then(AgentView::from_json),
        })
    }

    fn age(&self, w: &World, name: &str, secs: i64) {
        let db = db_path(w);
        let conn = Connection::open(&db).unwrap_or_else(|e| panic!("open {}: {e}", db.display()));
        let ids = "(SELECT id FROM tasks WHERE name = ?2)";
        for sql in [
            format!("UPDATE events SET ts = ts - ?1 WHERE task_id IN {ids}"),
            format!("UPDATE sessions SET created_at = created_at - ?1 WHERE task_id IN {ids}"),
            "UPDATE tasks SET updated_at = updated_at - ?1, created_at = created_at - ?1 \
             WHERE name = ?2"
                .to_string(),
        ] {
            shift_ts(&conn, &sql, secs, name);
        }
        shift_hook_cache(&w.state_dir(), secs);
    }

    fn run_bg(&self, w: &World, cwd: &Path, name: &str, command: &str) -> std::process::Child {
        let args = ["run", name, "--", command].map(String::from);
        self.spawn_bg(w, cwd, &args)
    }

    fn sleep_cmd(&self, w: &World, cwd: &Path, name: &str) -> Outcome<TaskView> {
        self.run(w, cwd, &name_args("sleep", name), TaskView::from_json)
    }

    fn wake_cmd(&self, w: &World, cwd: &Path, name: &str) -> Outcome<TaskView> {
        self.run(w, cwd, &name_args("wake", name), TaskView::from_json)
    }

    fn report(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        req: &super::ReportReq,
    ) -> Outcome<super::ReportRow> {
        self.report_impl(w, cwd, name, req)
    }

    fn reports(&self, w: &World, cwd: &Path, name: &str) -> Outcome<Vec<super::ReportRow>> {
        self.reports_impl(w, cwd, name)
    }

    fn version(&self, w: &World) -> Outcome<String> {
        self.version_impl(w)
    }

    fn github_setup(
        &self,
        w: &World,
        cwd: &Path,
        opts: &super::github_ops::GithubSetupOpts,
    ) -> Outcome<super::github_ops::GithubStore> {
        self.github_setup_impl(w, cwd, opts)
    }

    fn github_login(&self, w: &World, cwd: &Path) -> Outcome<super::github_ops::GithubStore> {
        self.github_login_impl(w, cwd)
    }

    fn github_raw(
        &self,
        w: &World,
        cwd: &Path,
        args: &[&str],
    ) -> Outcome<super::github_ops::GithubStore> {
        self.github_raw_impl(w, cwd, args)
    }

    fn proxy_check(&self, w: &World, cwd: &Path, name: &str) -> Outcome<super::ProxyCheck> {
        self.proxy_check_impl(w, cwd, name)
    }

    fn proxy_endpoint(&self, w: &World, name: &str) -> Option<super::ProxyEndpoint> {
        self.proxy_endpoint_impl(w, name)
    }
}
