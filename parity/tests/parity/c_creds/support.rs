use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use parity::adapter::{
    EnvOpts, EnvReport, EventKind, ExitClass, Outcome, StartOpts, TaskState, TaskView,
};
use parity::assert::assert_never_called;
use parity::fake::{claude, crabbox, herdr, Call, Rule};
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

pub(super) struct Fx {
    pub(super) w: World,
    pub(super) repo: Repo,
}

pub(super) fn fx() -> Fx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    Fx { w, repo }
}

pub(super) fn write_file(path: &Path, body: &str, mode: u32) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

impl Fx {
    pub(super) fn user(&self, doc: Value) {
        write_file(&self.w.config_path(), &doc.to_string(), 0o644);
    }

    pub(super) fn repo_cfg(&self, doc: Value) {
        write_file(&self.repo.path.join(".moor.yaml"), &doc.to_string(), 0o644);
    }

    pub(super) fn env_with(&self, opts: &EnvOpts) -> Outcome<EnvReport> {
        moor().env(&self.w, &self.repo.path, opts)
    }

    pub(super) fn env(&self) -> Outcome<EnvReport> {
        self.env_with(&EnvOpts::default())
    }

    pub(super) fn secret_file(&self, name: &str, body: &str) -> String {
        let path = self.w.root().join("secrets").join(name);
        write_file(&path, body, 0o600);
        path.to_string_lossy().into_owned()
    }

    pub(super) fn secret(&self, tag: &str) -> String {
        let secret = format!("SECRET-{tag}-{}", self.w.task_name("v"));
        self.w.register_secret(&secret);
        secret
    }
}

pub(super) fn assert_rejected(o: &Outcome<EnvReport>, what: &str) {
    assert_eq!(o.class, ExitClass::Failed, "{what}: {}", o.stderr);
    assert!(o.stdout.trim().is_empty(), "{what}: stdout {}", o.stdout);
    assert!(!o.stderr.trim().is_empty(), "{what}: silent failure");
}

pub(super) fn assert_accepted(o: &Outcome<EnvReport>, what: &str) {
    assert_eq!(o.class, ExitClass::Ok, "{what}: {}", o.stderr);
    o.ok();
}

pub(super) fn user_rejects(f: &Fx, docs: &[Value]) {
    for doc in docs {
        f.user(doc.clone());
        assert_rejected(&f.env(), &doc.to_string());
    }
}

pub(super) fn repo_rejects(f: &Fx, docs: &[Value]) {
    for doc in docs {
        f.repo_cfg(doc.clone());
        assert_rejected(&f.env(), &doc.to_string());
    }
}

pub(super) fn plain_dir(f: &Fx) -> std::path::PathBuf {
    let dir = f.w.root().join("plain");
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

pub(super) fn json_of(o: &Outcome<EnvReport>) -> Value {
    serde_json::from_str(&o.stdout).unwrap_or_else(|e| panic!("env stdout is not JSON: {e}"))
}

pub(super) fn flow_fakes(w: &World) {
    crabbox::install(w);
    herdr::install(w);
    claude::install(w);
    w.add_rules(
        "ssh",
        [Rule::any()
            .exec("cd '{box}' && {last}")
            .rewrite("/tmp/", "{box}/tmp/")
            .capture_stdin()],
    );
    let ssh_bin = w.tool_path("ssh");
    w.prepend_rules(
        "crabbox",
        [
            Rule::on(&["run"])
                .contains("MOOR_WORKDIR")
                .stdout("MOOR_WORKDIR={box}/work\n"),
            Rule::on(&["run"])
                .exec("{post}")
                .rewrite("/tmp/", "{box}/tmp/")
                .stderr("running on 192.0.2.10 {post}\n"),
            Rule::on(&["ssh"]).stdout(format!("'{}' 'box-host'\n", ssh_bin.display())),
            Rule::on(&["tunnel"])
                .stdout("{\"box\":1,\"host\":1}\n")
                .block(),
        ],
    );
}

pub(super) fn wait_proxy_gone(w: &World) {
    let dir = w.state_dir().join("proxy");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        let any_json = std::fs::read_dir(&dir).is_ok_and(|d| {
            d.flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "json"))
        });
        if !any_json {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("proxy supervisor did not exit after close");
}

pub(super) fn start_task(
    f: &Fx,
    name: &str,
    goal: Option<&str>,
    location: Option<&str>,
) -> Outcome<TaskView> {
    let opts = StartOpts {
        goal: goal.map(str::to_string),
        location: location.map(str::to_string),
        ..Default::default()
    };
    moor().start(&f.w, &f.repo.path, name, &opts)
}

pub(super) fn finish(f: &Fx, name: &str) {
    let closed = moor().close(&f.w, &f.repo.path, name, false);
    assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
    wait_proxy_gone(&f.w);
}

pub(super) fn cred_dirs(w: &World) -> Vec<String> {
    std::fs::read_dir(w.box_root().join("tmp"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("moor-cred-"))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn probe_file(task: &TaskView, f: &Fx, name: &str) -> String {
    let wt = task.worktree.clone().unwrap_or_default();
    [
        PathBuf::from(wt).join(name),
        f.w.box_root().join("work").join(name),
    ]
    .iter()
    .find_map(|p| std::fs::read_to_string(p).ok())
    .unwrap_or_else(|| panic!("setup probe file {name} not written"))
    .trim()
    .to_string()
}

pub(super) fn run_calls(w: &World) -> Vec<Call> {
    w.calls("crabbox")
        .into_iter()
        .filter(|c| c.argv.first().map(String::as_str) == Some("run"))
        .collect()
}

pub(super) const SETUP_PROBE: &str = concat!(
    "printf %s \"$TOK_V\" | wc -c > got-env; ",
    "wc -c < \"$KEYF_PATH\" > got-file; ",
    "stat -c %a \"$KEYF_PATH\" > got-filemode; ",
    "stat -c %a \"$(dirname \"$KEYF_PATH\")\" > got-dirmode"
);

pub(super) fn creds_config(f: &Fx, tok: &str, keyf: &str) {
    let (t, k) = (f.secret_file("tok", tok), f.secret_file("keyf", keyf));
    f.user(json!({ "credentials": { "TOK": { "file": t }, "KEYF": { "file": k } } }));
}

pub(super) fn pair_credentials() -> Value {
    json!({ "TOK": { "env": "TOK_V" }, "KEYF": { "file_env": "KEYF_PATH" } })
}

pub(super) fn bind_listing(w: &World, wt: &Path, branch: &str) {
    let body = json!({ "result": { "worktrees": [{
        "branch": branch, "label": "bound", "open_workspace_id": "ws-bound", "path": wt,
    }] } });
    w.prepend_rules(
        "herdr",
        [Rule::on(&["worktree", "list"]).stdout(body.to_string())],
    );
}

pub(super) fn started_ok(f: &Fx, name: &str, goal: Option<&str>) -> TaskView {
    let o = start_task(f, name, goal, None);
    assert_eq!(o.class, ExitClass::Ok, "{}{}", o.stdout, o.stderr);
    o.ok().clone()
}

pub(super) fn assert_rejected_start(f: &Fx, o: &Outcome<TaskView>, name: &str) {
    assert_eq!(o.class, ExitClass::Failed, "{}", o.stdout);
    assert!(!o.stderr.trim().is_empty());
    let state = o.value.as_ref().map(|t| t.state.clone());
    assert_eq!(state, Some(TaskState::Rejected));
    assert_never_called(&f.w.all_calls(), "crabbox", &["warmup"]);
    assert_never_called(&f.w.all_calls(), "herdr", &["worktree", "create"]);
    let kinds: Vec<EventKind> = moor()
        .events(&f.w, name)
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(kinds.contains(&EventKind::Rejected), "{kinds:?}");
}

pub(super) fn box_fx() -> Fx {
    let f = fx();
    flow_fakes(&f.w);
    f.repo_cfg(json!({ "agent": { "location": "box" } }));
    f.w.prepend_rules(
        "claude",
        [Rule::any().exec("cp '{flag:--mcp-config}' '{root}/mcp-seen' 2>/dev/null; true")],
    );
    f
}

pub(super) fn run_box_agent(w: &World) -> Option<String> {
    let seen = w.root().join("mcp-seen");
    let _ = std::fs::remove_file(&seen);
    let pane = herdr::calls_of(w, "pane", "run")
        .last()
        .and_then(|c| c.argv.last().cloned())
        .expect("herdr pane run recorded");
    let path = format!("{}:/usr/bin:/bin", w.bin_dir().display());
    w.command("sh")
        .env_clear()
        .env("PATH", path)
        .env("HOME", w.home())
        .env("PARITY_FAKES", w.fakes_dir())
        .arg("-c")
        .arg(pane)
        .output()
        .expect("run pane command");
    std::fs::read_to_string(seen).ok()
}

pub(super) fn repo_snapshot(repo: &Path) -> Vec<(String, Vec<u8>)> {
    let out = std::process::Command::new("find")
        .arg(repo)
        .args(["-path", "*/.git", "-prune", "-o", "-type", "f", "-print"])
        .output()
        .expect("find");
    let mut files: Vec<(String, Vec<u8>)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| (l.to_string(), std::fs::read(l).unwrap_or_default()))
        .collect();
    files.sort();
    files
}
