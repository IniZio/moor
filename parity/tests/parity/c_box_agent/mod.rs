use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parity::adapter::github_ops::GithubSetupOpts;
use parity::adapter::{EventKind, ExitClass, Outcome, StartOpts, TaskState, TaskView};
use parity::assert::{assert_never_called, assert_order, assert_secret_absent};
use parity::fake::{claude, crabbox, herdr, Call, Rule};
use parity::github::{FakeGithub, Reply, RepoFixture, TEST_KEY_PEM};
use parity::parity;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

const DUMP: &str = concat!(
    "env > '{root}/agent-env.txt'; pwd > '{root}/agent-cwd.txt'; ",
    "find '{box}/tmp' -printf '%m %p\\n' | sort > '{root}/agent-find.txt'; ",
    "cat .moor/agent-state.json > '{root}/agent-state.txt'"
);
const CLIENT_SECRET: &str = "SECRET-clientsecret-parity";

struct Bx {
    w: World,
    repo: Repo,
    gh: FakeGithub,
    fx: RepoFixture,
    github: bool,
}

fn bx_with(github: bool, app: bool) -> Bx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    let gh = FakeGithub::start();
    gh.attach(&w);
    if github {
        let key = format!("url.{}.insteadOf", repo.bare.display());
        let st = w
            .command("git")
            .args(["config", "--global", "--unset", &key])
            .status()
            .expect("git config");
        assert!(st.success());
    }
    let fx = RepoFixture::new("acme", "widget");
    gh.script_repo(&fx);
    flow_fakes(&w);
    agent_rule(&w, "");
    let b = Bx {
        w,
        repo,
        gh,
        fx,
        github,
    };
    for s in [
        &b.fx.bot_token,
        &b.fx.probe_token,
        &b.fx.scoped_token,
        &b.fx.user_access,
        &b.fx.user_refresh,
    ] {
        b.w.register_secret(s);
    }
    b.w.register_secret(CLIENT_SECRET);
    b.w.register_secret(TEST_KEY_PEM.lines().nth(1).expect("pem body"));
    repo_cfg(&b, json!({}));
    set_user(&b, json!({}));
    if app {
        setup_app(&b);
    }
    b.gh.clear_requests();
    b
}

fn bx(github: bool) -> Bx {
    bx_with(github, github)
}

fn write_file(path: &Path, body: &str, mode: u32) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

fn mode_of(p: &Path) -> u32 {
    std::fs::metadata(p).map_or(0, |m| m.permissions().mode() & 0o777)
}

fn setup_app(b: &Bx) {
    let key = b.w.root().join("app-key.pem");
    write_file(&key, TEST_KEY_PEM, 0o600);
    let opts = GithubSetupOpts {
        app_id: Some("4242".into()),
        client_id: Some(b.fx.client_id.clone()),
        key: Some(key),
        repo: Some("acme/widget".into()),
        secret_env: Some(CLIENT_SECRET.into()),
        ..Default::default()
    };
    let o = moor().github_setup(&b.w, &b.repo.path, &opts);
    assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
}

fn set_user(b: &Bx, extra: Value) {
    let mut doc = if b.github {
        json!({ "github": { "identity": "bot" } })
    } else {
        json!({})
    };
    for (k, v) in extra.as_object().expect("object") {
        doc[k] = v.clone();
    }
    write_file(&b.w.config_path(), &doc.to_string(), 0o644);
}

fn repo_cfg(b: &Bx, extra: Value) {
    let mut doc = json!({});
    for (k, v) in extra.as_object().expect("object") {
        match (k.as_str(), v.as_object()) {
            ("agent", Some(agent)) => {
                for (ak, av) in agent {
                    doc["agent"][ak] = av.clone();
                }
            }
            _ => doc[k] = v.clone(),
        }
    }
    write_file(&b.repo.path.join(".moor.yaml"), &doc.to_string(), 0o644);
}

fn flow_fakes(w: &World) {
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

fn agent_rule(w: &World, tail: &str) {
    w.prepend_rules(
        "claude",
        [Rule::any()
            .contains("--dangerously-skip-permissions")
            .exec(format!("{DUMP}{tail}"))],
    );
}

fn agent_exit(w: &World, code: i32) {
    w.prepend_rules(
        "claude",
        [Rule::any()
            .contains("--dangerously-skip-permissions")
            .exec(DUMP)
            .exit(code)],
    );
}

fn start_opts(goal: Option<&str>, model: Option<&str>) -> StartOpts {
    StartOpts {
        goal: goal.map(str::to_string),
        model: model.map(str::to_string),
        ..Default::default()
    }
}

fn start_with(b: &Bx, name: &str, opts: &StartOpts) -> Outcome<TaskView> {
    moor().start(&b.w, &b.repo.path, name, opts)
}

fn go(b: &Bx, name: &str, goal: &str) -> Outcome<TaskView> {
    start_with(b, name, &start_opts(Some(goal), None))
}

fn go_ok(b: &Bx, name: &str, goal: &str) -> Outcome<TaskView> {
    let o = go(b, name, goal);
    assert_eq!(o.class, ExitClass::Ok, "{}{}", o.stdout, o.stderr);
    assert_eq!(o.ok().state, TaskState::Working, "{}", o.stdout);
    o
}

fn doc(o: &Outcome<TaskView>) -> Value {
    serde_json::from_str(&o.stdout).unwrap_or(Value::Null)
}

fn reason_of(o: &Outcome<TaskView>) -> String {
    doc(o)["reason"].as_str().unwrap_or_default().to_string()
}

fn wait_proxy_gone(w: &World) {
    let dir = w.state_dir().join("proxy");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let any_json = std::fs::read_dir(&dir).is_ok_and(|d| {
            d.flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "json"))
        });
        if !any_json {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("proxy supervisor did not exit after close");
}

fn finish(b: &Bx, name: &str) {
    let closed = moor().close(&b.w, &b.repo.path, name, false);
    assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
    wait_proxy_gone(&b.w);
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn listing(root: &Path) -> Vec<String> {
    let mut out = vec![format!("{:o} .", mode_of(root))];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(root).expect("under root").display();
            out.push(format!("{:o} {rel}", mode_of(&p)));
            if p.is_dir() {
                stack.push(p);
            }
        }
    }
    out.sort();
    out
}

fn cred_dirs(w: &World) -> Vec<PathBuf> {
    std::fs::read_dir(w.box_root().join("tmp"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("moor-cred-"))
        })
        .collect()
}

fn cred_dir(w: &World) -> PathBuf {
    let dirs = cred_dirs(w);
    assert_eq!(dirs.len(), 1, "expected one credential dir: {dirs:?}");
    dirs[0].clone()
}

struct Launched {
    nonce: String,
    files: Vec<String>,
    brief: String,
    shim: Option<String>,
    box_port: u16,
}

fn tunnel_ports(w: &World) -> (u16, u16) {
    let call = crabbox::call(w, "tunnel").expect("tunnel recorded");
    let at = call
        .argv
        .iter()
        .position(|a| a == "--reverse")
        .expect("--reverse");
    let (b, h) = call.argv[at + 1].split_once(':').expect("B:H");
    (b.parse().expect("box port"), h.parse().expect("host port"))
}

fn launched(b: &Bx, name: &str, goal: &str) -> Launched {
    go_ok(b, name, goal);
    let dir = cred_dir(&b.w);
    let read = |rel: &str| std::fs::read_to_string(dir.join(rel)).ok();
    Launched {
        nonce: read("proxy-nonce").expect("proxy-nonce").trim().to_string(),
        files: listing(&dir),
        brief: read("brief.md").expect("brief.md"),
        shim: read("bin/gh"),
        box_port: tunnel_ports(&b.w).0,
    }
}

struct Ran {
    code: i32,
    text: String,
    env: BTreeMap<String, String>,
    find: String,
    state: Value,
}

fn pane_command(w: &World) -> String {
    herdr::calls_of(w, "pane", "run")
        .last()
        .and_then(|c| c.argv.last().cloned())
        .expect("herdr pane run recorded")
}

fn ran(b: &Bx) -> Ran {
    let root = b.w.root();
    for f in ["env", "cwd", "find", "state"] {
        let _ = std::fs::remove_file(root.join(format!("agent-{f}.txt")));
    }
    let path = format!("{}:/usr/bin:/bin", b.w.bin_dir().display());
    let out =
        b.w.command("sh")
            .env_clear()
            .env("PATH", path)
            .env("HOME", b.w.home())
            .env("PARITY_FAKES", b.w.fakes_dir())
            .arg("-c")
            .arg(pane_command(&b.w))
            .output()
            .expect("run pane command");
    let read =
        |f: &str| std::fs::read_to_string(root.join(format!("agent-{f}.txt"))).unwrap_or_default();
    let env = read("env")
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Ran {
        code: out.status.code().unwrap_or(-1),
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        env,
        find: read("find"),
        state: serde_json::from_str(&read("state")).unwrap_or(Value::Null),
    }
}

fn exit_marker(text: &str) -> Option<i32> {
    let at = text.rfind("moor: agent exited ")?;
    text[at + 19..].split_whitespace().next()?.parse().ok()
}

fn workdir(w: &World) -> PathBuf {
    w.box_root().join("work")
}

fn agent_call(w: &World) -> Call {
    w.calls("claude")
        .into_iter()
        .rfind(|c| c.argv.iter().any(|a| a == "--dangerously-skip-permissions"))
        .expect("box agent claude call")
}

fn arg_after(c: &Call, flag: &str) -> Option<String> {
    let at = c.argv.iter().position(|a| a == flag)?;
    c.argv.get(at + 1).cloned()
}

fn has(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

fn assert_nonce_confined(w: &World, nonce: &str) {
    assert!(!nonce.is_empty(), "nonce must be non-empty");
    let want = parity::fake::digest(nonce);
    let mut leaks: Vec<String> = Vec::new();
    for c in w.all_calls() {
        if c.argv.iter().any(|a| a.contains(nonce)) {
            leaks.push(format!("argv of {} #{}", c.tool, c.seq));
        }
        let in_env =
            c.env.values().any(|v| v.contains(nonce)) || c.env_digests.values().any(|d| *d == want);
        if in_env && c.tool != "claude" {
            leaks.push(format!("env of {} #{}", c.tool, c.seq));
        }
    }
    for (i, t) in w.transcript().iter().enumerate() {
        if t.contains(nonce) {
            leaks.push(format!("moor output #{i}"));
        }
    }
    let state = w.state_dir();
    let held = |f: &Path| std::fs::read(f).is_ok_and(|b| has(&b, nonce.as_bytes()));
    for f in files_under(&state) {
        let rel = f.strip_prefix(&state).unwrap_or(&f).to_path_buf();
        if !rel.starts_with("proxy") && held(&f) {
            leaks.push(format!("state/{}", rel.display()));
        }
    }
    for f in files_under(&w.box_root()) {
        if held(&f) {
            let rel = f
                .strip_prefix(w.box_root())
                .unwrap_or(&f)
                .display()
                .to_string();
            leaks.push(format!("box/{rel}"));
        }
    }
    assert!(
        leaks.is_empty(),
        "nonce leaked into:\n  {}",
        leaks.join("\n  ")
    );
}

fn ssh_stdin_has(w: &World, secret: &str) -> bool {
    w.calls("ssh").iter().any(|c| c.stdin.contains(secret))
}

fn count(w: &World, tool: &str, verb: &[&str]) -> usize {
    w.calls(tool)
        .iter()
        .filter(|c| c.argv.len() >= verb.len() && c.argv[..verb.len()] == *verb)
        .count()
}

mod env;
mod gh_shim;
mod launch;
mod relaunch;
mod setup;
