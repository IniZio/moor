use std::path::{Path, PathBuf};
use std::process::Stdio;

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::{
    exit_class, AgentView, CloseView, EnvOpts, EnvReport, Event, EventKind, ExitClass, Moor,
    OpenView, Outcome, PaneView, RemoveView, Session, StartOpts, TaskView, WaitView,
};
use crate::world::World;

#[path = "old_reports.rs"]
mod old_reports;

pub struct OldCli {
    pub bin: PathBuf,
}

pub fn seam_env(w: &World) -> Vec<(String, String)> {
    let config = w.root().join("agent-config.json");
    if !config.exists() {
        let doc = serde_json::json!({
            "projects": { w.root().to_string_lossy(): { "hasTrustDialogAccepted": true } }
        });
        std::fs::write(&config, doc.to_string()).expect("write agent-config.json");
    }
    let path = |p: PathBuf| p.to_string_lossy().into_owned();
    let pairs = [
        ("MOOR_CRABBOX", path(w.tool_path("crabbox"))),
        ("MOOR_HERDR_BIN", path(w.tool_path("herdr"))),
        ("MOOR_AGENT_BIN", path(w.tool_path("claude"))),
        ("MOOR_AGENT_CONFIG", path(config)),
        (
            "MOOR_AGENT_DAEMON_LOG",
            path(w.home().join(".claude/daemon.log")),
        ),
        ("MOOR_WAIT_POLL_MS", "50".to_string()),
        ("MOOR_AGENT_PROMPT_WINDOW_MS", "0".to_string()),
        ("MOOR_AGENT_SETTLE_TIMEOUT_MS", "5000".to_string()),
        ("MOOR_GITHUB_API", "http://127.0.0.1:9".to_string()),
        ("MOOR_GITHUB_WEB", "http://127.0.0.1:9".to_string()),
    ];
    let preset = w.base_env();
    pairs
        .into_iter()
        .filter(|(k, _)| !k.starts_with("MOOR_GITHUB_") || !preset.iter().any(|(p, _)| p == k))
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(1)
}

impl OldCli {
    fn spawn_bg(&self, w: &World, cwd: &Path, args: &[String]) -> std::process::Child {
        let child = w
            .command(&self.bin)
            .envs(seam_env(w))
            .current_dir(cwd)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", self.bin.display()));
        w.track_pid(child.id());
        child
    }

    fn run_unit(&self, w: &World, cwd: &Path, args: &[String]) -> Outcome<()> {
        let mut o = self.run(w, cwd, args, |_| ());
        if o.class == ExitClass::Ok {
            o.value = Some(());
        }
        o
    }

    fn run<T>(
        &self,
        w: &World,
        cwd: &Path,
        args: &[String],
        parse: impl Fn(&Value) -> T,
    ) -> Outcome<T> {
        self.run_env(w, cwd, args, &EnvOpts::default(), parse)
    }

    fn run_env<T>(
        &self,
        w: &World,
        cwd: &Path,
        args: &[String],
        env: &EnvOpts,
        parse: impl Fn(&Value) -> T,
    ) -> Outcome<T> {
        let mut cmd = w.command(&self.bin);
        cmd.envs(seam_env(w)).envs(env.set_env.iter().cloned());
        for k in &env.unset_env {
            cmd.env_remove(k);
        }
        let child = cmd
            .current_dir(cwd)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", self.bin.display()));
        w.track_pid(child.id());
        let out = child.wait_with_output().expect("wait for moor");
        let exit = exit_code(out.status);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        w.log_transcript(format!("moor {}\n{stdout}\n{stderr}", args.join(" ")));
        let json: Option<Value> = serde_json::from_str(&stdout).ok();
        let warnings = json
            .as_ref()
            .and_then(|j| j.get("warnings"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Outcome {
            exit,
            class: exit_class(exit),
            value: json.as_ref().map(parse),
            stdout,
            stderr,
            warnings,
        }
    }
}

fn open_registry(w: &World) -> Option<Connection> {
    let db = w.state_dir().join("registry.db");
    db.exists().then(|| {
        Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|e| panic!("open {}: {e}", db.display()))
    })
}

fn name_args(verb: &str, name: &str) -> Vec<String> {
    vec![verb.to_string(), name.to_string(), "--json".to_string()]
}

fn shift_ts(conn: &Connection, sql: &str, secs: i64, name: &str) {
    conn.execute(sql, rusqlite::params![secs, name])
        .unwrap_or_else(|e| panic!("age registry: {e}"));
}

fn shift_hook_cache(state: &Path, secs: i64) {
    let mut stack = vec![state.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let is_cache = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with("agent-state-last.json"));
            if !is_cache {
                continue;
            }
            let Ok(mut doc) = serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap())
            else {
                continue;
            };
            if let Some(at) = doc["read_at"].as_i64() {
                doc["read_at"] = Value::from(at - secs);
                std::fs::write(&path, doc.to_string()).expect("rewrite hook cache");
            }
        }
    }
}

fn str_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Set `box.provider` in the user config (JSON, which old moor reads as YAML).
fn merge_box_provider(path: &Path, provider: &str) {
    let mut doc: Value = match std::fs::read_to_string(path) {
        Ok(s) => {
            serde_json::from_str(&s).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => panic!("read {}: {e}", path.display()),
    };
    let root = doc
        .as_object_mut()
        .unwrap_or_else(|| panic!("{} is not a JSON object", path.display()));
    let b = root
        .entry("box")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .unwrap_or_else(|| panic!("{}: `box` is not an object", path.display()));
    b.insert("provider".into(), Value::String(provider.to_string()));
    std::fs::write(path, doc.to_string())
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn start_args(w: &World, name: &str, opts: &StartOpts) -> Vec<String> {
    if opts.size.is_some() {
        panic!("old adapter: size is a repo .moor.yaml setting; set it via RepoConfig");
    }
    if let Some(provider) = &opts.provider {
        merge_box_provider(&w.config_path(), provider);
    }
    let mut args: Vec<String> = vec!["start".into(), name.into(), "--json".into()];
    let mut flag = |f: &str, v: Option<String>| {
        if let Some(v) = v {
            args.push(f.into());
            args.push(v);
        }
    };
    flag("--issue", opts.issue.clone());
    flag("--goal", opts.goal.clone());
    flag("--model", opts.model.clone());
    flag(
        "--worktree",
        opts.worktree
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    flag("--agent-location", opts.location.clone());
    args
}

impl Moor for OldCli {
    fn name(&self) -> &'static str {
        "old"
    }

    fn start(&self, w: &World, cwd: &Path, name: &str, opts: &StartOpts) -> Outcome<TaskView> {
        let args = start_args(w, name, opts);
        let mut o = self.run(w, cwd, &args, |j| {
            TaskView::from_json(j.get("task").unwrap_or(&Value::Null))
        });
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
        if let Some(l) = &opts.agent_location {
            args.push("--agent-location".into());
            args.push(l.clone());
        }
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
        let child = w
            .command(&self.bin)
            .envs(seam_env(w))
            .current_dir(cwd)
            .args(start_args(w, name, opts))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", self.bin.display()));
        w.track_pid(child.id());
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
}

const RUN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

fn crabbox_run_calls(w: &World) -> usize {
    w.calls("crabbox")
        .iter()
        .filter(|c| c.argv.first().map(String::as_str) == Some("run"))
        .count()
}

fn read_all(mut r: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

fn db_path(w: &World) -> PathBuf {
    w.state_dir().join("registry.db")
}

impl OldCli {
    fn spawn_wait(
        &self,
        w: &World,
        cwd: &Path,
        args: &[String],
        interrupt: bool,
    ) -> (i32, String, String) {
        let before = crabbox_run_calls(w);
        let mut child = w
            .command(&self.bin)
            .envs(seam_env(w))
            .current_dir(cwd)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", self.bin.display()));
        let pid = child.id();
        w.track_pid(pid);
        let out = read_all(child.stdout.take().expect("stdout pipe"));
        let err = read_all(child.stderr.take().expect("stderr pipe"));
        let start = std::time::Instant::now();
        let mut signalled = !interrupt;
        let status = loop {
            if let Some(st) = child.try_wait().expect("poll moor") {
                break st;
            }
            if !signalled && crabbox_run_calls(w) > before {
                unsafe { libc::kill(pid as i32, libc::SIGINT) };
                signalled = true;
            }
            if start.elapsed() > RUN_DEADLINE {
                let _ = child.kill();
                let _ = child.wait();
                panic!("moor {args:?} did not finish within {RUN_DEADLINE:?}");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let (stdout, stderr) = (
            out.join().unwrap_or_default(),
            err.join().unwrap_or_default(),
        );
        w.log_transcript(format!("moor {}\n{stdout}\n{stderr}", args.join(" ")));
        (exit_code(status), stdout, stderr)
    }

    fn task_events(&self, w: &World, name: &str) -> Vec<Event> {
        self.events(w, name)
    }
}

fn plain<T>(exit: i32, stdout: String, stderr: String, value: Option<T>) -> Outcome<T> {
    Outcome {
        exit,
        class: exit_class(exit),
        stdout,
        stderr,
        warnings: Vec::new(),
        value,
    }
}

impl OldCli {
    pub(super) fn run_impl(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        argv: &[&str],
        opts: &super::RunOpts,
    ) -> Outcome<super::RunResult> {
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
            .map(|e| super::RunResult {
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
    ) -> Outcome<super::GateVerdict> {
        let mut args: Vec<String> = ["gate", "check", sha, "--name", name, "--json"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        if let Some(c) = command {
            args.push("--command".into());
            args.push(c.into());
        }
        let (exit, stdout, stderr) = self.spawn_wait(w, cwd, &args, false);
        let value = serde_json::from_str::<Value>(&stdout)
            .ok()
            .map(|j| super::GateVerdict {
                ok: j["ok"].as_bool().unwrap_or(false),
                sha: j["sha"].as_str().map(str::to_string),
                exit: j["exit"].as_i64().map(|n| n as i32),
                task: j["task"].as_str().map(str::to_string),
            });
        plain(exit, stdout, stderr, value)
    }

    pub(super) fn runs_impl(&self, w: &World, name: &str) -> Vec<super::RunRow> {
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
                Ok(super::RunRow {
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

    pub(super) fn seed_run_impl(&self, w: &World, row: &super::RunRow) {
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
