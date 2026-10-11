use std::path::{Path, PathBuf};
use std::process::Stdio;

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::{exit_class, CloseView, Event, ExitClass, Moor, Outcome, StartOpts, TaskView};
use crate::world::World;

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
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(1)
}

impl OldCli {
    fn run<T>(
        &self,
        w: &World,
        cwd: &Path,
        args: &[String],
        parse: impl Fn(&Value) -> T,
    ) -> Outcome<T> {
        let child = w
            .command(&self.bin)
            .envs(seam_env(w))
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

impl Moor for OldCli {
    fn name(&self) -> &'static str {
        "old"
    }

    fn start(&self, w: &World, cwd: &Path, name: &str, opts: &StartOpts) -> Outcome<TaskView> {
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
        let db = w.state_dir().join("registry.db");
        if !db.exists() {
            return Vec::new();
        }
        let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|e| panic!("open {}: {e}", db.display()));
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
                    kind,
                    payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                })
            })
            .expect("query events");
        rows.map(|r| r.expect("event row")).collect()
    }
}
