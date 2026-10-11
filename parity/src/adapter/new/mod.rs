//! Parity adapter for the rewritten `moor` binary (design §10.4). Only
//! `version` decodes a value; other mapped verbs run the CLI and drop the
//! value. A command the binary reports as not implemented, a missing envelope
//! class, or an unmapped seam panics, so the case fails instead of passing
//! on a coincidental error class.
mod exec;
mod store;

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::github_ops::{GithubSetupOpts, GithubStore};
use super::{
    CloseView, EnvOpts, EnvReport, Event, GateVerdict, Moor, OpenView, Outcome, PaneView,
    ProxyCheck, ProxyEndpoint, RemoveView, ReportReq, ReportRow, RunOpts, RunResult, RunRow,
    Session, StartOpts, TaskView, WaitView,
};
use crate::world::World;

pub struct NewCli {
    pub bin: PathBuf,
}

fn sv(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

impl NewCli {
    /// Run a mapped verb; the decoded value is intentionally dropped.
    fn go<T>(&self, w: &World, cwd: &Path, args: Vec<String>) -> Outcome<T> {
        exec::retype(exec::invoke(&self.bin, w, cwd, &args, None), |_| None)
    }
}

impl Moor for NewCli {
    fn name(&self) -> &'static str {
        "new"
    }

    fn caps(&self) -> &'static [&'static str] {
        &[
            "VersionHash",
            "CloseGate",
            "MoorWarning",
            "RestartClosed",
            "EnvPreflightScoped",
        ]
    }

    fn start(&self, w: &World, cwd: &Path, name: &str, opts: &StartOpts) -> Outcome<TaskView> {
        let o = exec::invoke(&self.bin, w, cwd, &start_args(name, opts), None);
        exec::retype(o, |v| v.get("task").map(task_view))
    }

    fn list(&self, w: &World, cwd: &Path) -> Outcome<Vec<TaskView>> {
        let o = exec::invoke(&self.bin, w, cwd, &sv(&["show"]), None);
        exec::retype(o, |v| {
            Some(v.get("tasks")?.as_array()?.iter().map(task_view).collect())
        })
    }

    fn close(&self, w: &World, cwd: &Path, name: &str, force: bool) -> Outcome<CloseView> {
        let mut a = sv(&["close", name]);
        if force {
            a.push("--force".into());
        }
        let o = exec::invoke(&self.bin, w, cwd, &a, None);
        exec::retype(o, |v| {
            let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
            Some(CloseView {
                task: task_view(v.get("task")?),
                box_deleted: flag("box_deleted"),
                workspace_closed: flag("workspace_closed"),
                sessions_stopped: Vec::new(),
            })
        })
    }

    fn events(&self, w: &World, name: &str) -> Vec<Event> {
        store::events(w, name)
    }

    fn env(&self, w: &World, cwd: &Path, _opts: &EnvOpts) -> Outcome<EnvReport> {
        self.go(w, cwd, sv(&["env"]))
    }

    fn remove(&self, w: &World, cwd: &Path, name: &str, discard: bool) -> Outcome<RemoveView> {
        let mut a = sv(&["remove", name]);
        if discard {
            a.push("--discard".into());
        }
        self.go(w, cwd, a)
    }

    fn sessions(&self, w: &World, name: &str) -> Vec<Session> {
        store::sessions(w, name)
    }

    fn age(&self, _w: &World, _name: &str, _secs: i64) {
        exec::unmapped("age")
    }

    fn start_detached(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        opts: &StartOpts,
    ) -> std::process::Child {
        exec::spawn(&self.bin, w, cwd, &start_args(name, opts))
    }

    fn run(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        argv: &[&str],
        opts: &RunOpts,
    ) -> Outcome<RunResult> {
        let mut a = sv(&["run", name]);
        if opts.no_sync {
            a.push("--no-sync".into());
        }
        if let Some(r) = &opts.record {
            a.extend(sv(&["--record", r]));
        }
        a.push("--".into());
        a.extend(argv.iter().map(|s| s.to_string()));
        self.go(w, cwd, a)
    }

    fn cp(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        box_path: &str,
        host_path: &Path,
    ) -> Outcome<()> {
        let hp = host_path.to_string_lossy();
        self.go(w, cwd, sv(&["cp", name, box_path, &hp]))
    }

    fn gate_check(
        &self,
        w: &World,
        cwd: &Path,
        sha: &str,
        name: &str,
        command: Option<&str>,
    ) -> Outcome<GateVerdict> {
        let mut a = sv(&["gate", "check", sha, name]);
        if let Some(c) = command {
            a.extend(sv(&["--command", c]));
        }
        self.go(w, cwd, a)
    }

    fn runs(&self, _w: &World, _name: &str) -> Vec<RunRow> {
        exec::unmapped("runs")
    }

    fn seed_run(&self, _w: &World, _row: &RunRow) {
        exec::unmapped("seed_run")
    }

    fn wait(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        timeout_secs: Option<u64>,
    ) -> Outcome<WaitView> {
        let mut a = sv(&["wait", name]);
        if let Some(t) = timeout_secs {
            a.extend(sv(&["--timeout", &t.to_string()]));
        }
        self.go(w, cwd, a)
    }

    fn steer(&self, w: &World, cwd: &Path, name: &str, message: &str) -> Outcome<()> {
        self.go(w, cwd, sv(&["steer", name, message]))
    }

    fn open(&self, w: &World, cwd: &Path, name: &str) -> Outcome<OpenView> {
        self.go(w, cwd, sv(&["open", name]))
    }

    fn shell(&self, w: &World, cwd: &Path, name: &str) -> Outcome<()> {
        self.go(w, cwd, sv(&["shell", name]))
    }

    fn pane(&self, w: &World, cwd: &Path, name: &str) -> Outcome<PaneView> {
        self.go(w, cwd, sv(&["show", name, "--pane", "--events", "50"]))
    }

    fn run_bg(&self, w: &World, cwd: &Path, name: &str, command: &str) -> std::process::Child {
        exec::spawn(
            &self.bin,
            w,
            cwd,
            &sv(&["run", name, "--", "sh", "-c", command]),
        )
    }

    fn sleep_cmd(&self, w: &World, cwd: &Path, name: &str) -> Outcome<TaskView> {
        self.go(w, cwd, sv(&["sleep", name]))
    }

    fn wake_cmd(&self, w: &World, cwd: &Path, name: &str) -> Outcome<TaskView> {
        self.go(w, cwd, sv(&["wake", name]))
    }

    fn report(&self, w: &World, cwd: &Path, name: &str, req: &ReportReq) -> Outcome<ReportRow> {
        let mut a = sv(&["report", name, &req.kind]);
        if let Some(f) = &req.file {
            a.extend(sv(&["--file", &f.to_string_lossy()]));
        }
        if let Some(l) = &req.link {
            a.extend(sv(&["--link", l]));
        }
        if let Some(t) = &req.title {
            a.extend(sv(&["--title", t]));
        }
        self.go(w, cwd, a)
    }

    fn reports(&self, w: &World, cwd: &Path, name: &str) -> Outcome<Vec<ReportRow>> {
        self.go(w, cwd, sv(&["show", name, "--reports"]))
    }

    fn version(&self, w: &World) -> Outcome<String> {
        let o = exec::invoke(&self.bin, w, w.root(), &sv(&["version"]), None);
        exec::retype(o, |v| {
            let s = |k: &str| {
                v.get(k)
                    .map(|x| x.to_string().trim_matches('"').to_string())
            };
            Some(format!(
                "moor {} {} schema={}",
                s("version")?,
                s("commit")?,
                s("schema")?
            ))
        })
    }

    fn github_setup(&self, w: &World, cwd: &Path, _opts: &GithubSetupOpts) -> Outcome<GithubStore> {
        self.go(w, cwd, sv(&["github", "setup"]))
    }

    fn github_login(&self, w: &World, cwd: &Path) -> Outcome<GithubStore> {
        self.go(w, cwd, sv(&["github", "login"]))
    }

    fn github_raw(&self, _w: &World, _cwd: &Path, _args: &[&str]) -> Outcome<GithubStore> {
        exec::unmapped("github_raw")
    }

    fn proxy_check(&self, w: &World, cwd: &Path, name: &str) -> Outcome<ProxyCheck> {
        self.go(w, cwd, sv(&["proxy", "check", name]))
    }

    fn proxy_endpoint(&self, _w: &World, _name: &str) -> Option<ProxyEndpoint> {
        exec::unmapped("proxy_endpoint")
    }
}

/// Design key `workspace` feeds parity's `workspace_id`.
fn task_view(v: &Value) -> TaskView {
    let mut v = v.clone();
    if let Some(ws) = v.get("workspace").cloned() {
        v["workspace_id"] = ws;
    }
    TaskView::from_json(&v)
}

fn start_args(name: &str, opts: &StartOpts) -> Vec<String> {
    let mut a = sv(&["start", name]);
    let mut flag = |f: &str, v: Option<String>| {
        if let Some(v) = v {
            a.extend([f.to_string(), v]);
        }
    };
    flag("--goal", opts.goal.clone());
    flag("--issue", opts.issue.clone());
    flag(
        "--worktree",
        opts.worktree
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    flag("--model", opts.model.clone());
    a
}
