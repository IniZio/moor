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
    let mut doc = json!({ "agent": { "location": "box" } });
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

fn start_opts(goal: Option<&str>, location: Option<&str>, model: Option<&str>) -> StartOpts {
    StartOpts {
        goal: goal.map(str::to_string),
        location: location.map(str::to_string),
        model: model.map(str::to_string),
        ..Default::default()
    }
}

fn start_with(b: &Bx, name: &str, opts: &StartOpts) -> Outcome<TaskView> {
    moor().start(&b.w, &b.repo.path, name, opts)
}

fn go(b: &Bx, name: &str, goal: &str) -> Outcome<TaskView> {
    start_with(b, name, &start_opts(Some(goal), None, None))
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

parity!(
    box_launch_proxies_then_delivers_then_opens_agent_tab,
    covers = ["A-39", "C-97"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("order");
        let o = go_ok(&b, &name, "build it");
        let task = o.ok().clone();
        let calls = b.w.all_calls();
        assert_order(
            &calls,
            &[
                ("crabbox", &["warmup"]),
                ("crabbox", &["tunnel", "--reverse"]),
                ("crabbox", &["ssh"]),
                ("ssh", &["box-host"]),
                ("herdr", &["tab", "create"]),
                ("herdr", &["pane", "run"]),
            ],
        );
        let slug = crabbox::slug(&b.w).expect("slug");
        let tunnel = crabbox::call(&b.w, "tunnel").expect("tunnel");
        assert_eq!(arg_after(&tunnel, "--id").as_deref(), Some(slug.as_str()));
        let (box_port, host_port) = tunnel_ports(&b.w);
        assert!(box_port > 0 && host_port > 0);
        let tab = herdr::calls_of(&b.w, "tab", "create").remove(0);
        assert_eq!(
            arg_after(&tab, "--workspace"),
            task.workspace_id,
            "agent tab opens in the Task Workspace"
        );
        assert_eq!(arg_after(&tab, "--label"), Some(format!("agent:{name}")));
        assert_eq!(arg_after(&tab, "--cwd"), task.worktree);
        let sessions = moor().sessions(&b.w, &name);
        assert_eq!(sessions.len(), 1, "{sessions:?}");
        assert!(sessions[0].box_pane && sessions[0].live);
        let run = herdr::calls_of(&b.w, "pane", "run").remove(0);
        assert_eq!(run.argv[2], sessions[0].session_id);
        let started: Vec<_> = moor()
            .events(&b.w, &name)
            .into_iter()
            .filter(|e| e.kind == EventKind::SessionStarted)
            .collect();
        assert_eq!(started.len(), 1);
        assert_never_called(&calls, "claude", &["--bg"]);
        assert_eq!(
            cred_dirs(&b.w).len(),
            1,
            "files delivered before the pane opens"
        );
        finish(&b, &name);
    }
);

parity!(
    box_agent_without_goal_launches_nothing,
    covers = ["C-110"],
    tags = [],
    {
        let b = bx(false);
        let name = b.w.task_name("nogoal");
        let o = start_with(&b, &name, &start_opts(None, None, None));
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        let calls = b.w.all_calls();
        assert_never_called(&calls, "crabbox", &["tunnel"]);
        assert_never_called(&calls, "herdr", &["tab", "create"]);
        assert_never_called(&calls, "herdr", &["pane", "run"]);
        assert!(b.w.calls("ssh").is_empty(), "nothing delivered");
        assert!(moor().sessions(&b.w, &name).is_empty());
        assert!(cred_dirs(&b.w).is_empty());
        let proxy = b.w.state_dir().join("proxy");
        assert!(
            !proxy.exists() || files_under(&proxy).is_empty(),
            "no proxy without an agent"
        );
        finish(&b, &name);
    }
);

parity!(
    box_launch_delivers_only_nonce_brief_and_shim_with_private_modes,
    covers = ["C-98"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("files");
        let l = launched(&b, &name, "deliver");
        let want = [
            "600 brief.md",
            "600 proxy-nonce",
            "700 .",
            "700 bin",
            "700 bin/gh",
        ];
        assert_eq!(l.files, want, "credential dir layout and modes");
        assert!(!l.nonce.is_empty() && !l.nonce.contains(char::is_whitespace));
        let shim = l.shim.expect("shim delivered for a github.com origin");
        assert!(shim.starts_with("#!/bin/sh"), "{shim}");
        assert!(l.brief.contains("# Task"), "{}", l.brief);
        assert!(
            ssh_stdin_has(&b.w, &l.nonce),
            "nonce travels over ssh stdin"
        );
        assert!(
            ssh_stdin_has(&b.w, "MOOR_GH_API"),
            "shim travels over ssh stdin"
        );
        for c in b.w.all_calls() {
            assert!(
                !c.argv.iter().any(|a| a.contains(&l.nonce)),
                "argv of {}",
                c.tool
            );
        }
        for token in [&b.fx.bot_token, &b.fx.probe_token, &b.fx.scoped_token] {
            assert!(!ssh_stdin_has(&b.w, token), "github token in ssh stdin");
            assert_secret_absent(&b.w, token, &[]);
        }
        b.w.assert_no_new_tmp_creds();
        finish(&b, &name);
    }
);

parity!(
    box_agent_pane_runs_one_line_and_removes_cred_dir_on_any_exit,
    covers = ["C-99", "C-106", "REQ-35"],
    tags = [],
    {
        let b = bx(false);
        for (tag, code, kill) in [("ok", 0, false), ("three", 3, false), ("kill", 0, true)] {
            if kill {
                agent_rule(&b.w, "; kill -9 $$");
            } else {
                agent_exit(&b.w, code);
            }
            let name = b.w.task_name(tag);
            launched(&b, &name, "exit codes");
            let r = ran(&b);
            let marker = exit_marker(&r.text);
            if kill {
                assert!(marker.is_some_and(|m| m != 0), "{tag}: {}", r.text);
            } else {
                assert_eq!(marker, Some(code), "{tag}: {}", r.text);
            }
            assert!(cred_dirs(&b.w).is_empty(), "{tag}: {:?}", cred_dirs(&b.w));
            finish(&b, &name);
        }
        b.w.assert_no_new_tmp_creds();
    }
);

parity!(
    box_agent_gets_proxy_nonce_never_a_real_credential,
    covers = ["C-100", "C-111", "C-104", "C-106"],
    tags = [],
    {
        let b = bx(true);
        let claude_token = format!("SECRET-claude-{}", b.w.task_name("v"));
        let file = b.w.root().join("secrets/claude");
        write_file(&file, &claude_token, 0o600);
        set_user(
            &b,
            json!({ "credentials": { "CLAUDE_CODE_OAUTH_TOKEN": { "file": file } } }),
        );
        let name = b.w.task_name("nonce");
        let l = launched(&b, &name, "no secret in the Box");
        let r = ran(&b);
        assert_eq!(r.code, 0, "{}", r.text);
        assert_eq!(
            r.env.get("ANTHROPIC_BASE_URL"),
            Some(&format!("http://127.0.0.1:{}", l.box_port))
        );
        assert_eq!(r.env.get("CLAUDE_CODE_OAUTH_TOKEN"), Some(&l.nonce));
        for var in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
        ] {
            assert!(!r.env.contains_key(var), "{var} reached the agent");
        }
        assert!(
            !r.find.contains("proxy-nonce"),
            "nonce file is gone before claude runs"
        );
        assert_nonce_confined(&b.w, &l.nonce);
        let pem_line = TEST_KEY_PEM.lines().nth(1).expect("pem body");
        let host_secrets = [
            claude_token.as_str(),
            CLIENT_SECRET,
            pem_line,
            &b.fx.bot_token,
            &b.fx.probe_token,
            &b.fx.scoped_token,
            &b.fx.user_access,
            &b.fx.user_refresh,
        ];
        let env_text = r.env.values().cloned().collect::<Vec<_>>().join("\n");
        for s in host_secrets {
            assert_secret_absent(&b.w, s, &["github/"]);
            assert!(!ssh_stdin_has(&b.w, s), "{s} in ssh stdin");
            assert!(!env_text.contains(s), "{s} in agent env");
            assert!(!pane_command(&b.w).contains(s), "{s} in pane command");
        }
        finish(&b, &name);
        assert_nonce_confined(&b.w, &l.nonce);
    }
);

fn origin(b: &Bx, url: Option<&str>) {
    let git = |args: &[&str]| {
        let st =
            b.w.command("git")
                .current_dir(&b.repo.path)
                .args(args)
                .status()
                .expect("git");
        assert!(st.success(), "git {args:?}");
    };
    match url {
        Some(u) => git(&["remote", "set-url", "origin", u]),
        None => git(&["remote", "remove", "origin"]),
    }
}

fn leaves_git_config_or_credential_on_box(w: &World) -> bool {
    w.box_root().join("home/.gitconfig").exists()
        || files_under(&w.box_root()).iter().any(|f| {
            std::fs::read_to_string(f)
                .is_ok_and(|t| t.contains("credential") || t.contains("x-access-token"))
        })
}

parity!(
    box_agent_git_env_routes_github_through_proxy_and_exports_push_url,
    covers = ["C-101", "REQ-34"],
    tags = [],
    {
        let b = bx(true);
        let bare = b.repo.bare.display().to_string();
        let gh = "https://github.com/acme/widget";
        let cases: [(&str, Option<&str>, Option<String>); 5] = [
            ("https", Some(gh), Some(gh.into())),
            (
                "sshurl",
                Some("ssh://git@github.com/acme/widget.git"),
                Some(format!("{gh}.git")),
            ),
            (
                "scp",
                Some("git@github.com:acme/widget.git"),
                Some(format!("{gh}.git")),
            ),
            ("local", Some(&bare), Some(bare.clone())),
            ("none", None, None),
        ];
        for (tag, url, want_push) in cases {
            origin(&b, url);
            let name = b.w.task_name(tag);
            let l = launched(&b, &name, "push env");
            let r = ran(&b);
            let e = &r.env;
            assert_eq!(
                e.get("GIT_CONFIG_COUNT").map(String::as_str),
                Some("1"),
                "{tag}"
            );
            let key = format!(
                "url.http://127.0.0.1:{}/t/{}/github/.insteadOf",
                l.box_port, l.nonce
            );
            assert_eq!(e.get("GIT_CONFIG_KEY_0"), Some(&key), "{tag}");
            assert_eq!(
                e.get("GIT_CONFIG_VALUE_0").map(String::as_str),
                Some("https://github.com/"),
                "{tag}"
            );
            assert_eq!(e.get("MOOR_PUSH_URL"), want_push.as_ref(), "{tag}");
            assert!(
                !e.contains_key("GIT_CONFIG_KEY_1"),
                "{tag}: no credential helper"
            );
            assert!(
                !leaves_git_config_or_credential_on_box(&b.w),
                "{tag}: git auth left a file on the Box"
            );
            finish(&b, &name);
        }
    }
);

parity!(
    box_agent_gh_env_puts_shim_first_and_names_canonical_repo_only_when_different,
    covers = ["C-102"],
    tags = [],
    {
        let b = bx(true);
        let repo_path = b.fx.repo_path();
        for (tag, name_in_api, canonical) in [
            ("same", "widget", None),
            ("case", "WIDGET", None),
            ("moved", "Gadget", Some("acme/Gadget")),
        ] {
            let mut repo = b.fx.repo_json();
            repo["name"] = json!(name_in_api);
            b.gh.reroute("GET", &repo_path, vec![Reply::ok(&repo)]);
            let name = b.w.task_name(tag);
            let l = launched(&b, &name, "gh env");
            let r = ran(&b);
            let e = &r.env;
            let api = format!("http://127.0.0.1:{}/t/{}/github-api", l.box_port, l.nonce);
            assert_eq!(e.get("MOOR_GH_API"), Some(&api), "{tag}");
            assert_eq!(
                e.get("MOOR_GH_REPO").map(String::as_str),
                Some("acme/widget"),
                "{tag}"
            );
            assert_eq!(
                e.get("MOOR_GH_REPO_CANONICAL").map(String::as_str),
                canonical,
                "{tag}"
            );
            let first = e["PATH"].split(':').next().unwrap_or_default();
            assert!(
                first.ends_with("/bin") && first.contains("moor-cred-"),
                "{tag}: {first}"
            );
            assert!(
                !e.contains_key("GH_TOKEN") && !e.contains_key("GITHUB_TOKEN"),
                "{tag}"
            );
            assert!(
                r.find.contains("/bin/gh"),
                "{tag}: shim present while the agent runs"
            );
            finish(&b, &name);
        }
    }
);

parity!(
    box_agent_without_github_origin_gets_proxy_and_git_env_but_no_shim,
    covers = ["C-119", "C-101", "C-102"],
    tags = [],
    {
        let b = bx(false);
        let name = b.w.task_name("plain");
        let l = launched(&b, &name, "no github");
        assert_eq!(l.files, ["600 brief.md", "600 proxy-nonce", "700 ."]);
        assert!(l.shim.is_none());
        let r = ran(&b);
        for var in ["MOOR_GH_API", "MOOR_GH_REPO", "MOOR_GH_REPO_CANONICAL"] {
            assert!(!r.env.contains_key(var), "{var} without a GitHub origin");
        }
        assert!(
            !r.env["PATH"].contains("moor-cred-"),
            "no shim directory on PATH"
        );
        assert_eq!(
            r.env.get("ANTHROPIC_BASE_URL"),
            Some(&format!("http://127.0.0.1:{}", l.box_port))
        );
        assert_eq!(r.env.get("GIT_CONFIG_COUNT").map(String::as_str), Some("1"));
        assert!(r.env.contains_key("GIT_CONFIG_KEY_0"));
        for word in ["shim", "MOOR_GH", "`gh"] {
            assert!(!l.brief.contains(word), "brief mentions {word}");
        }
        assert!(
            b.gh.requests().is_empty(),
            "no mint for a non-GitHub origin"
        );
        finish(&b, &name);
    }
);

parity!(
    box_agent_applies_run_env_and_path_before_proxy_and_shim_path,
    covers = ["C-103"],
    tags = [],
    {
        let b = bx(true);
        repo_cfg(
            &b,
            json!({ "run": { "env": { "PARITY_RUN": "it's set" }, "path": ["/opt/parity-tools"] } }),
        );
        let name = b.w.task_name("prefix");
        launched(&b, &name, "run prefix");
        let r = ran(&b);
        assert_eq!(
            r.env.get("PARITY_RUN").map(String::as_str),
            Some("it's set")
        );
        let parts: Vec<&str> = r.env["PATH"].split(':').collect();
        let at = |needle: &str| {
            parts
                .iter()
                .position(|p| p.contains(needle))
                .unwrap_or_else(|| panic!("{needle} missing from PATH {parts:?}"))
        };
        assert!(at("moor-cred-") < at(".local/bin"));
        assert!(at(".local/bin") < at(".bun/bin"));
        assert!(at(".bun/bin") < at("/opt/parity-tools"));
        finish(&b, &name);
    }
);

parity!(
    box_agent_claude_runs_with_bypass_settings_and_brief_file,
    covers = ["C-104", "C-105", "REQ-39", "REQ-33"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("claude");
        let l = launched(&b, &name, "GOALTEXT-claude-argv");
        let r = ran(&b);
        let wd = workdir(&b.w);
        let call = agent_call(&b.w);
        assert_eq!(
            call.argv,
            [
                "--dangerously-skip-permissions".to_string(),
                "--settings".to_string(),
                format!("{}/.moor/agent-hooks.json", wd.display()),
                "Read and follow .moor/brief.md".to_string(),
            ]
        );
        assert_eq!(Path::new(&call.cwd), wd.as_path());
        let brief = wd.join(".moor/brief.md");
        assert_eq!(std::fs::read_to_string(&brief).unwrap_or_default(), l.brief);
        assert_eq!(mode_of(&brief), 0o600);
        let hooks: Value = serde_json::from_str(
            &std::fs::read_to_string(wd.join(".moor/agent-hooks.json")).expect("hooks file"),
        )
        .expect("hooks json");
        for (event, state) in [
            ("UserPromptSubmit", "working"),
            ("Stop", "idle"),
            ("Notification", "waiting_input"),
        ] {
            let cmd = hooks["hooks"][event][0]["hooks"][0]["command"]
                .as_str()
                .unwrap_or_default();
            assert!(
                cmd.contains("agent-state.json") && cmd.contains(state),
                "{event}: {cmd}"
            );
        }
        assert_eq!(r.state["state"], "starting", "state reset at launch");
        assert_eq!(r.env.get("CLAUDE_CODE_OAUTH_TOKEN"), Some(&l.nonce));
        assert!(r.env["ANTHROPIC_BASE_URL"].starts_with("http://127.0.0.1:"));
        assert_order(
            &b.w.all_calls(),
            &[
                ("crabbox", &["tunnel"]),
                ("claude", &["--dangerously-skip-permissions"]),
            ],
        );
        finish(&b, &name);
    }
);

parity!(
    box_agent_model_flag_only_when_given_and_host_agents_never_bypass,
    covers = ["C-104"],
    tags = [],
    {
        let b = bx(false);
        let mut seen: Vec<Option<String>> = Vec::new();
        let runs = [
            ("bare", json!({}), None),
            ("flag", json!({}), Some("flag-model")),
            (
                "repo",
                json!({ "agent": { "model": "repo-model" } }),
                Some("flag-model"),
            ),
        ];
        for (tag, cfg, flag) in runs {
            repo_cfg(&b, cfg);
            let name = b.w.task_name(tag);
            let opts = start_opts(Some("model"), None, flag);
            assert_eq!(start_with(&b, &name, &opts).class, ExitClass::Ok);
            ran(&b);
            seen.push(arg_after(&agent_call(&b.w), "--model"));
            finish(&b, &name);
        }
        assert_eq!(
            seen,
            [
                None,
                Some("flag-model".to_string()),
                Some("repo-model".to_string())
            ]
        );
        repo_cfg(&b, json!({ "agent": { "location": "host" } }));
        let host = b.w.task_name("host");
        let o = go(&b, &host, "host agent");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let bg: Vec<Call> =
            b.w.calls("claude")
                .into_iter()
                .filter(|c| c.argv.iter().any(|a| a == "--bg"))
                .collect();
        assert_eq!(bg.len(), 1, "host agent dispatched through claude --bg");
        assert!(!bg[0]
            .argv
            .iter()
            .any(|a| a == "--dangerously-skip-permissions"));
        finish(&b, &host);
    }
);

parity!(
    box_brief_is_a_file_naming_the_reports_path_without_host_paths_or_moor_commands,
    covers = ["D-28", "REQ-32", "REQ-34", "C-105"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("brief");
        let marker = format!("GOALMARK-{}", b.w.task_name("g"));
        let l = launched(&b, &name, &marker);
        assert!(l.brief.contains(&marker));
        assert!(l.brief.contains(".moor/reports/"), "{}", l.brief);
        for bad in [
            "moor run",
            "moor report",
            "moor steer",
            "`moor ",
            "--state-dir",
        ] {
            assert!(!l.brief.contains(bad), "brief contains {bad}");
        }
        let rows = moor().list(&b.w, &b.repo.path).ok().clone();
        let mut hosts = vec![
            b.w.root().display().to_string(),
            b.w.state_dir().display().to_string(),
            b.repo.path.display().to_string(),
            b.w.home().display().to_string(),
        ];
        hosts.extend(rows.iter().filter_map(|t| t.worktree.clone()));
        for h in hosts {
            assert!(!l.brief.contains(&h), "brief names host path {h}");
        }
        for word in ["shim", "MOOR_GH", "`gh"] {
            assert!(!l.brief.contains(word), "brief mentions {word}");
        }
        assert!(l.brief.contains("MOOR_PUSH_URL"), "{}", l.brief);
        assert!(
            l.brief.contains(&format!("moor/{name}")),
            "Task branch named"
        );
        assert!(l.brief.contains("hook"), "defers to repo hooks");
        for c in b.w.all_calls() {
            assert!(
                !c.argv.iter().any(|a| a.contains(&marker)),
                "brief text in argv of {}",
                c.tool
            );
        }
        assert!(!pane_command(&b.w).contains(&marker));
        let r = ran(&b);
        assert!(
            !r.text.contains(&marker),
            "brief text reached the pane scrollback"
        );
        finish(&b, &name);
    }
);

parity!(
    box_agent_preseeds_claude_first_run_state_for_the_workdir,
    covers = ["REQ-33"],
    tags = [],
    {
        let b = bx(false);
        let home_cfg = b.w.box_root().join("home/.claude.json");
        let wd = workdir(&b.w).display().to_string();
        for (tag, seeded) in [("fresh", false), ("merge", true)] {
            let _ = std::fs::remove_file(&home_cfg);
            if seeded {
                let seed = r#"{"keep":true,"projects":{"/other":{"a":1}}}"#;
                write_file(&home_cfg, seed, 0o600);
            }
            let name = b.w.task_name(tag);
            launched(&b, &name, "preseed");
            let r = ran(&b);
            assert_eq!(r.code, 0, "{}", r.text);
            let text = std::fs::read_to_string(&home_cfg).expect("~/.claude.json");
            let doc: Value = serde_json::from_str(&text).expect("json");
            assert_eq!(doc["hasCompletedOnboarding"], true, "{tag}");
            assert!(doc["theme"].is_string(), "{tag}");
            assert_eq!(
                doc["projects"][&wd]["hasTrustDialogAccepted"], true,
                "{tag}"
            );
            if seeded {
                assert_eq!(doc["keep"], true);
                assert_eq!(doc["projects"]["/other"]["a"], 1);
            }
            finish(&b, &name);
        }
    }
);

parity!(
    box_start_reports_first_run_prompt_without_failing,
    covers = ["A-40", "REQ-33"],
    tags = [],
    {
        let b = bx(false);
        let kinds = [
            (
                "trust",
                "Do you trust the files in this folder?",
                "folder_trust",
            ),
            ("enter", "Press Enter to continue", "press_enter"),
            ("notes", "Security notes:", "security_notes"),
            (
                "theme",
                "Choose the text style that looks best with your terminal",
                "theme",
            ),
        ];
        for (tag, line, kind) in kinds {
            herdr::pane_text(&b.w, &format!("welcome\n{line}\n"));
            let name = b.w.task_name(tag);
            let o = go(&b, &name, "prompt");
            assert_eq!(o.class, ExitClass::Ok, "{tag}: {}", o.stderr);
            assert_eq!(o.ok().state, TaskState::Working, "{tag}");
            let reason = reason_of(&o);
            let pane = &moor().sessions(&b.w, &name)[0].session_id;
            assert!(
                reason.contains(line) && reason.contains(kind) && reason.contains(pane.as_str()),
                "{tag}: {reason}"
            );
            finish(&b, &name);
        }
    }
);

parity!(
    box_start_with_quiet_or_unreadable_pane_still_succeeds_without_a_prompt_reason,
    covers = ["A-40"],
    tags = [],
    {
        let b = bx(false);
        herdr::pane_text(&b.w, "Welcome to Claude\n> \n");
        let quiet = b.w.task_name("quiet");
        let o = go(&b, &quiet, "no prompt");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(reason_of(&o), "");
        finish(&b, &quiet);
        b.w.prepend_rules(
            "herdr",
            [Rule::on(&["pane", "read"]).exit(1).stderr("no pane\n")],
        );
        let broken = b.w.task_name("broken");
        let o = go(&b, &broken, "read error");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        assert_eq!(reason_of(&o), "");
        assert_eq!(moor().sessions(&b.w, &broken).len(), 1, "agent started");
        finish(&b, &broken);
    }
);

parity!(
    box_dispatch_failures_leave_the_task_working_and_a_rerun_retries,
    covers = ["A-41"],
    tags = [],
    {
        let b = bx(false);
        b.w.prepend_rules(
            "herdr",
            [Rule::on(&["tab", "create"])
                .exit(1)
                .stderr("tab refused\n")
                .times(1)],
        );
        let name = b.w.task_name("tabfail");
        let o = go(&b, &name, "dispatch");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        let reason = reason_of(&o);
        assert!(
            reason.contains("agent dispatch failed") && reason.contains("re-run start"),
            "{reason}"
        );
        assert!(moor().sessions(&b.w, &name).is_empty());
        let again = go(&b, &name, "dispatch");
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        assert_eq!(reason_of(&again), "");
        assert_eq!(moor().sessions(&b.w, &name).len(), 1);
        finish(&b, &name);

        crabbox::deny(&b.w, "ssh", 1);
        let tabs = count(&b.w, "herdr", &["tab", "create"]);
        let blocked = b.w.task_name("nossh");
        let o = go(&b, &blocked, "dispatch");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        assert!(
            reason_of(&o).contains("agent dispatch failed"),
            "{}",
            o.stdout
        );
        assert_eq!(
            count(&b.w, "herdr", &["tab", "create"]),
            tabs,
            "no pane without a channel"
        );
        assert!(moor().sessions(&b.w, &blocked).is_empty());
        finish(&b, &blocked);
    }
);

fn delivery_dir(c: &Call) -> Option<String> {
    let cmd = c.argv.last()?;
    let start = cmd.find("mkdir -p '")? + 10;
    let end = start + cmd[start..].find('\'')?;
    Some(cmd[start..end].trim_end_matches('/').to_string())
}

parity!(
    box_delivery_failure_removes_the_cred_dir_and_names_a_failed_cleanup,
    covers = ["B-116", "C-97", "A-41"],
    tags = [],
    {
        let b = bx(false);
        b.w.prepend_rules(
            "ssh",
            [Rule::any()
                .contains("brief.md")
                .exit(1)
                .stderr("disk full\n")],
        );
        let name = b.w.task_name("delfail");
        let o = go(&b, &name, "deliver");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        let reason = reason_of(&o);
        assert!(
            reason.contains("agent dispatch failed") && reason.contains("brief.md"),
            "{reason}"
        );
        let ssh = b.w.calls("ssh");
        let first = &ssh[0];
        let remote = first.argv.last().cloned().unwrap_or_default();
        assert!(remote.contains("umask 077"), "{remote}");
        let dir = delivery_dir(first).expect("delivery dir");
        let slug = crabbox::slug(&b.w).expect("slug");
        let leaf = dir.rsplit('/').next().unwrap_or_default();
        let hex = leaf
            .strip_prefix(&format!("moor-cred-{slug}-"))
            .expect("dir naming");
        assert!(
            hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit()),
            "{leaf}"
        );
        assert!(dir.starts_with("/tmp/"), "{dir}");
        let removal = ssh
            .last()
            .and_then(|c| c.argv.last())
            .cloned()
            .unwrap_or_default();
        assert!(
            removal.starts_with("rm -rf") && removal.contains(&dir),
            "{removal}"
        );
        assert!(
            cred_dirs(&b.w).is_empty(),
            "credential dir removed after the failure"
        );
        assert_never_called(&b.w.all_calls(), "herdr", &["pane", "run"]);
        finish(&b, &name);

        b.w.prepend_rules(
            "ssh",
            [Rule::any().contains("rm -rf").exit(1).stderr("busy\n")],
        );
        let stuck = b.w.task_name("rmfail");
        let o = go(&b, &stuck, "deliver");
        let reason = reason_of(&o);
        assert!(
            reason.contains("failed to remove box credential dir")
                && reason.contains("remove it by hand"),
            "{reason}"
        );
        assert_eq!(
            cred_dirs(&b.w).len(),
            1,
            "the dir the message names is still there"
        );
        finish(&b, &stuck);
    }
);

parity!(
    box_start_preflight_mints_and_discards_before_any_box_and_failure_is_retryable,
    covers = ["C-107"],
    tags = [],
    {
        let fresh = bx_with(true, false);
        let name = fresh.w.task_name("preflight");
        let o = go(&fresh, &name, "mint first");
        assert_eq!(o.class, ExitClass::Failed, "{}", o.stdout);
        assert!(
            o.stdout.trim().is_empty(),
            "stdout empty on preflight failure: {}",
            o.stdout
        );
        assert!(!o.stderr.trim().is_empty());
        let kinds: Vec<EventKind> = moor()
            .events(&fresh.w, &name)
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(kinds.contains(&EventKind::CredentialFailed), "{kinds:?}");
        let rows = moor().list(&fresh.w, &fresh.repo.path).ok().clone();
        assert_eq!(rows[0].state, TaskState::SetupFailed);
        assert_never_called(&fresh.w.all_calls(), "crabbox", &["warmup"]);
        assert_never_called(&fresh.w.all_calls(), "herdr", &["pane", "run"]);
        setup_app(&fresh);
        let o = go(&fresh, &name, "mint first");
        assert_eq!(o.class, ExitClass::Ok, "{}{}", o.stdout, o.stderr);
        assert_eq!(o.ok().state, TaskState::Working);
        assert_eq!(count(&fresh.w, "crabbox", &["warmup"]), 1);
        assert_eq!(count(&fresh.w, "herdr", &["pane", "run"]), 1);
        finish(&fresh, &name);

        let b = bx(true);
        let ok = b.w.task_name("minted");
        go_ok(&b, &ok, "mint first");
        assert!(scoped_mints(&b) >= 1, "a repo-scoped token was minted");
        for t in [&b.fx.probe_token, &b.fx.bot_token] {
            assert_secret_absent(&b.w, t, &["github/"]);
            assert!(!ssh_stdin_has(&b.w, t));
        }
        finish(&b, &ok);
    }
);

parity!(
    box_agent_starts_without_a_claude_token_configured,
    covers = ["C-109"],
    tags = [],
    {
        let b = bx(false);
        let name = b.w.task_name("notoken");
        let l = launched(&b, &name, "no token");
        let r = ran(&b);
        assert_eq!(r.code, 0, "{}", r.text);
        assert_eq!(r.env.get("CLAUDE_CODE_OAUTH_TOKEN"), Some(&l.nonce));
        assert_eq!(moor().sessions(&b.w, &name).len(), 1);
        finish(&b, &name);
    }
);

fn scoped_mints(b: &Bx) -> usize {
    let path = format!("/app/installations/{}/access_tokens", b.fx.installation_id);
    let scoped = |r: &parity::github::Request| r.body.contains("repository_ids");
    b.gh.requests_to("POST", &path)
        .iter()
        .filter(|r| scoped(r))
        .count()
}

fn live_panes(w: &World) {
    let panes: Vec<Value> = (1..=9)
        .map(|n| json!({ "pane_id": format!("p-{n}") }))
        .collect();
    let body = json!({ "result": { "panes": panes } }).to_string();
    w.prepend_rules("herdr", [Rule::on(&["pane", "list"]).stdout(body)]);
}

parity!(
    box_start_attaches_to_a_live_pane_and_ignores_the_new_goal,
    covers = ["A-48", "C-110"],
    tags = [],
    {
        let b = bx(false);
        let name = b.w.task_name("live");
        go_ok(&b, &name, "first goal");
        let again = go(&b, &name, "second goal");
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        let d = doc(&again);
        assert_eq!(d["attached"], true, "{d}");
        assert!(d["relaunched"].is_null(), "{d}");
        assert!(again.stderr.contains("--goal ignored"), "{}", again.stderr);
        assert_eq!(count(&b.w, "herdr", &["tab", "create"]), 1);
        assert_eq!(moor().sessions(&b.w, &name).len(), 1);
        assert_eq!(count(&b.w, "crabbox", &["tunnel"]), 1);
        finish(&b, &name);
    }
);

parity!(
    box_start_relaunches_a_gone_pane_with_new_goal_saving_the_transcript_first,
    covers = ["A-46", "A-47", "C-110"],
    tags = [],
    {
        let b = bx(false);
        crabbox::cp(&b.w);
        crabbox::run_in(&b.w, &workdir(&b.w));
        seed_transcript(&b.w);
        let name = b.w.task_name("gone");
        go_ok(&b, &name, "first goal");
        let old = moor().sessions(&b.w, &name)[0].session_id.clone();
        herdr::pane_gone(&b.w);
        let o = go(&b, &name, "second goal");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let d = doc(&o);
        assert_eq!(d["relaunched"]["goal"], "second goal", "{d}");
        assert_eq!(d["relaunched"]["old_pane"], old.as_str(), "{d}");
        let new = d["relaunched"]["new_pane"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(!new.is_empty() && new != old);
        let sessions = moor().sessions(&b.w, &name);
        let live: Vec<_> = sessions.iter().filter(|s| s.live).collect();
        assert_eq!(live.len(), 1, "{sessions:?}");
        assert_eq!(live[0].session_id, new);
        assert!(live[0].box_pane);
        assert_eq!(
            count(&b.w, "crabbox", &["tunnel"]),
            1,
            "relaunch reuses the proxy"
        );
        assert_order(
            &b.w.all_calls(),
            &[
                ("herdr", &["tab", "create"]),
                ("crabbox", &["run"]),
                ("crabbox", &["cp"]),
                ("herdr", &["tab", "create"]),
            ],
        );
        let archive =
            b.w.calls("crabbox")
                .into_iter()
                .find(|c| c.argv.iter().any(|a| a.contains("transcripts.tgz")))
                .expect("transcript archive run");
        assert!(archive.argv.iter().any(|a| a == "--no-sync"));
        finish(&b, &name);
    }
);

parity!(
    box_relaunch_uses_the_newest_goal_and_stays_in_the_box_even_if_config_says_host,
    covers = ["A-46"],
    tags = [],
    {
        let b = bx(false);
        live_panes(&b.w);
        let name = b.w.task_name("goals");
        go_ok(&b, &name, "goal one");
        herdr::pane_text(&b.w, "bye\nmoor: agent exited 3\n");
        let attach = |goal: Option<&str>, location: Option<&str>| {
            let o = start_with(&b, &name, &start_opts(goal, location, None));
            assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
            doc(&o)["relaunched"].clone()
        };
        let first = attach(None, None);
        assert_eq!(first["goal"], "goal one", "{first}");
        assert_eq!(first["old_pane"], "p-1");
        repo_cfg(&b, json!({ "agent": { "location": "host" } }));
        let second = attach(Some("goal two"), None);
        assert_eq!(second["goal"], "goal two", "{second}");
        assert_eq!(second["old_pane"], first["new_pane"]);
        let third = attach(None, None);
        assert_eq!(third["goal"], "goal two", "{third}");
        assert_eq!(count(&b.w, "herdr", &["tab", "create"]), 4);
        let bg =
            b.w.calls("claude")
                .iter()
                .any(|c| c.argv.iter().any(|a| a == "--bg"));
        assert!(!bg, "location stays Box");
        finish(&b, &name);
    }
);

parity!(
    box_probe_errors_count_as_a_live_pane_and_block_relaunch,
    covers = ["A-46", "A-48"],
    tags = [],
    {
        let b = bx(false);
        let name = b.w.task_name("flaky");
        go_ok(&b, &name, "first goal");
        b.w.prepend_rules(
            "herdr",
            [
                Rule::on(&["pane", "list"]).exit(1).stderr("herdr down\n"),
                Rule::on(&["pane", "read"]).exit(1).stderr("herdr down\n"),
            ],
        );
        let o = go(&b, &name, "second goal");
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        let d = doc(&o);
        assert!(d["relaunched"].is_null(), "{d}");
        assert_eq!(count(&b.w, "herdr", &["tab", "create"]), 1);
        assert_eq!(moor().sessions(&b.w, &name).len(), 1);
        finish(&b, &name);
    }
);

parity!(
    box_steer_on_an_exited_pane_names_the_start_command_to_fix_it,
    covers = ["REQ-35"],
    tags = [],
    {
        let b = bx(false);
        live_panes(&b.w);
        let name = b.w.task_name("steerdead");
        go_ok(&b, &name, "first goal");
        herdr::pane_text(&b.w, "bye\nmoor: agent exited 3\n");
        let o = moor().steer(&b.w, &b.repo.path, &name, "again");
        assert_eq!(o.class, ExitClass::Failed, "{}", o.stderr);
        for want in [name.as_str(), "start", "--goal", "3"] {
            assert!(o.stderr.contains(want), "{want} missing from: {}", o.stderr);
        }
        assert_never_called(&b.w.all_calls(), "herdr", &["pane", "send-text"]);
        let again = start_with(&b, &name, &start_opts(Some("again"), None, None));
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        assert_eq!(
            count(&b.w, "herdr", &["tab", "create"]),
            2,
            "new pane opened"
        );
        assert_eq!(
            moor()
                .sessions(&b.w, &name)
                .iter()
                .filter(|s| s.live)
                .count(),
            1
        );
        assert!(b
            .w
            .calls("claude")
            .iter()
            .all(|c| !c.argv.iter().any(|a| a == "--bg")));
        finish(&b, &name);
    }
);

fn seed_transcript(w: &World) {
    let wd = workdir(w).display().to_string();
    let enc: String = wd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let rel = format!("home/.claude/projects/{enc}/session1.jsonl");
    write_file(&w.box_root().join(rel), "{\"type\":\"user\"}\n", 0o644);
}

fn transcripts(w: &World) -> Vec<PathBuf> {
    files_under(&w.state_dir())
        .into_iter()
        .filter(|f| {
            f.to_string_lossy().contains("-transcript-")
                && f.extension().is_some_and(|e| e == "tgz")
        })
        .collect()
}

parity!(
    box_close_and_relaunch_save_the_transcript_0600_and_failure_never_blocks,
    covers = ["A-47", "REQ-37"],
    tags = [],
    {
        let b = bx(false);
        crabbox::cp(&b.w);
        crabbox::run_in(&b.w, &workdir(&b.w));
        seed_transcript(&b.w);
        let name = b.w.task_name("transcript");
        go_ok(&b, &name, "first goal");
        herdr::pane_gone(&b.w);
        assert!(!doc(&go(&b, &name, "second goal"))["relaunched"].is_null());
        let saved = transcripts(&b.w);
        assert_eq!(saved.len(), 1, "relaunch saved one archive: {saved:?}");
        assert_eq!(mode_of(&saved[0]), 0o600);
        let tar = b.w.command("tar").arg("tzf").arg(&saved[0]).output();
        let listed = String::from_utf8_lossy(&tar.expect("tar").stdout).into_owned();
        assert!(listed.contains("session1.jsonl"), "{listed}");
        let closed = moor().close(&b.w, &b.repo.path, &name, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        wait_proxy_gone(&b.w);
        let saved = transcripts(&b.w);
        assert_eq!(saved.len(), 2, "close saved a second archive: {saved:?}");
        assert!(saved.iter().all(|f| mode_of(f) == 0o600));
        let cp = crabbox::call(&b.w, "cp").expect("cp");
        assert!(cp
            .argv
            .iter()
            .any(|a| a.starts_with("SANDBOX:") && a.ends_with(".moor/transcripts.tgz")));
        assert_order(
            &b.w.all_calls(),
            &[("crabbox", &["cp"]), ("crabbox", &["stop"])],
        );

        std::fs::remove_dir_all(b.w.box_root().join("home/.claude")).expect("clear transcripts");
        let bare = b.w.task_name("notrans");
        go_ok(&b, &bare, "no transcript");
        let closed = moor().close(&b.w, &b.repo.path, &bare, false);
        assert_eq!(closed.class, ExitClass::Ok, "{}", closed.stderr);
        wait_proxy_gone(&b.w);
        assert_eq!(
            transcripts(&b.w).len(),
            2,
            "nothing saved without a transcript"
        );
        assert!(closed.ok().box_deleted, "close still released the Box");
    }
);

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| {
            s.rsplit(')')
                .next()
                .map(|t| t.trim_start().starts_with(['R', 'S', 'D']))
        })
        .unwrap_or(false)
}

parity!(
    box_close_ends_the_proxy_supervisor_and_its_tunnel,
    covers = ["A-96"],
    tags = [],
    {
        let b = bx(false);
        let name = b.w.task_name("proxyend");
        go_ok(&b, &name, "tunnel up");
        let tunnel = crabbox::call(&b.w, "tunnel").expect("tunnel");
        assert!(alive(tunnel.pid), "tunnel runs while the Task is open");
        finish(&b, &name);
        let deadline = Instant::now() + Duration::from_secs(10);
        while alive(tunnel.pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(tunnel.pid), "tunnel survived close");
    }
);

struct Shim {
    script: PathBuf,
    repo: PathBuf,
    curl_log: PathBuf,
    delivered: String,
}

fn shim(b: &Bx, name: &str) -> Shim {
    let l = launched(b, name, "shim");
    let delivered = l.shim.expect("shim delivered");
    let dir = b.w.root().join("shim");
    let script = dir.join("gh");
    write_file(&script, &delivered, 0o700);
    let curl_log = dir.join("curl.log");
    let wrapper = format!(
        "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done >> '{}'\nexec /usr/bin/curl \"$@\"\n",
        curl_log.display()
    );
    write_file(&dir.join("bin/curl"), &wrapper, 0o700);
    let repo = dir.join("checkout");
    std::fs::create_dir_all(&repo).expect("checkout dir");
    for args in [
        &["init", "-q", "-b", "feature/x"][..],
        &["commit", "-q", "--allow-empty", "-m", "init"][..],
    ] {
        let st =
            b.w.command("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .expect("git");
        assert!(st.success(), "git {args:?}");
    }
    Shim {
        script,
        repo,
        curl_log,
        delivered,
    }
}

struct Sh {
    code: i32,
    out: String,
    err: String,
}

impl Shim {
    fn invoke(&self, b: &Bx, args: &[&str], env: &[(String, String)]) -> Sh {
        let base = self.script.parent().expect("shim dir");
        let path = format!("{}:/usr/bin:/bin", base.join("bin").display());
        let mut cmd = b.w.command("sh");
        cmd.env_clear()
            .env("PATH", path)
            .env("HOME", b.w.home())
            .envs(env.iter().cloned())
            .current_dir(&self.repo)
            .arg(&self.script)
            .args(args);
        let out = cmd.output().expect("run shim");
        Sh {
            code: out.status.code().unwrap_or(-1),
            out: String::from_utf8_lossy(&out.stdout).into_owned(),
            err: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn run(&self, b: &Bx, args: &[&str], extra: &[(&str, &str)]) -> Sh {
        let mut env = vec![
            ("MOOR_GH_API".to_string(), b.gh.url()),
            ("MOOR_GH_REPO".to_string(), "acme/widget".to_string()),
        ];
        env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        self.invoke(b, args, &env)
    }

    fn without(&self, b: &Bx, args: &[&str], drop: &str) -> Sh {
        let env: Vec<(String, String)> = [
            ("MOOR_GH_API".to_string(), b.gh.url()),
            ("MOOR_GH_REPO".to_string(), "acme/widget".to_string()),
        ]
        .into_iter()
        .filter(|(k, _)| k != drop)
        .collect();
        self.invoke(b, args, &env)
    }

    fn curl_urls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.curl_log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("http"))
            .map(str::to_string)
            .collect()
    }
}

fn pulls(b: &Bx) -> String {
    format!("{}/pulls", b.fx.repo_path())
}

fn pr_reply(b: &Bx) {
    let body = json!({ "html_url": "https://github.com/acme/widget/pull/7", "number": 7 });
    b.gh.route("POST", &pulls(b), vec![Reply::json(201, &body)]);
}

fn sent(b: &Bx, method: &str, path: &str) -> Vec<parity::github::Request> {
    b.gh.requests_to(method, path)
}

fn parsed(s: &str) -> Option<Value> {
    serde_json::from_str(s).ok()
}

parity!(
    box_gh_shim_supports_only_pr_commands_and_validates_flags_before_any_request,
    covers = ["C-112", "C-113"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("surface");
        let s = shim(&b, &name);
        assert!(
            !s.delivered.contains("GH_TOKEN"),
            "shim never reads a token"
        );
        let refused: [&[&str]; 7] = [
            &["repo", "view"],
            &["api", "/user"],
            &["auth", "token"],
            &["pr", "merge", "1"],
            &[],
            &["pr"],
            &["pr", "close", "1"],
        ];
        for args in refused {
            let r = s.run(&b, args, &[]);
            assert_eq!(r.code, 2, "{args:?}: {}", r.err);
            assert!(r.err.contains("supports only"), "{args:?}: {}", r.err);
        }
        for drop in ["MOOR_GH_API", "MOOR_GH_REPO"] {
            let r = s.without(&b, &["pr", "list"], drop);
            assert_eq!(r.code, 2, "{drop}: {}", r.err);
            assert!(r.err.contains("MOOR_GH_API"), "{}", r.err);
        }
        let flag_errors: [(&[&str], &str); 4] = [
            (&["pr", "create", "--title"], "needs a value"),
            (&["pr", "create", "--bogus", "x"], "unsupported flag"),
            (&["pr", "view", "1", "2"], "unexpected argument"),
            (&["pr", "view", "abc"], "bad pull request number"),
        ];
        for (args, want) in flag_errors {
            let r = s.run(&b, args, &[]);
            assert_eq!(r.code, 2, "{args:?}: {}", r.err);
            assert!(r.err.contains(want), "{args:?}: {}", r.err);
        }
        assert!(
            b.gh.requests().iter().all(|r| !r.path.contains("/pulls")),
            "no request was made"
        );
        assert!(s.curl_urls().is_empty());

        pr_reply(&b);
        let forms: [&[&str]; 5] = [
            &["pr", "create", "--title", "T1", "--body", "B1"],
            &[
                "pr", "create", "-t", "T2", "-b", "B2", "-B", "dev", "-H", "feat2", "-d",
            ],
            &[
                "pr",
                "create",
                "--title=T3",
                "--body=B3",
                "--base=rel",
                "--head=feat3",
            ],
            &[
                "pr", "create", "--title", "T4", "--body", "B4", "--draft", "--base", "x",
                "--head", "y",
            ],
            &[
                "pr",
                "create",
                "-R",
                "acme/widget",
                "--title",
                "T5",
                "--body",
                "B5",
            ],
        ];
        for args in forms {
            let r = s.run(&b, args, &[]);
            assert_eq!(r.code, 0, "{args:?}: {}", r.err);
        }
        let reqs = sent(&b, "POST", &pulls(&b));
        let got: Vec<(String, String, bool)> = reqs
            .iter()
            .map(|r| {
                let j = r.json();
                let f = |k: &str| j[k].as_str().unwrap_or_default().to_string();
                (f("title"), f("base"), j["draft"].as_bool().unwrap_or(false))
            })
            .collect();
        assert_eq!(got[0], ("T1".into(), "main".into(), false));
        assert_eq!(got[1], ("T2".into(), "dev".into(), true));
        assert_eq!(got[2], ("T3".into(), "rel".into(), false));
        assert_eq!(got[3], ("T4".into(), "x".into(), true));
        assert_eq!(reqs[2].json()["head"], "feat3");
        assert_eq!(reqs[1].json()["body"], "B2");

        let body = json!({ "title": "T", "state": "open", "html_url": "U", "number": 5 });
        b.gh.route("GET", &format!("{}/5", pulls(&b)), vec![Reply::ok(&body)]);
        let views: [&[&str]; 2] = [
            &["pr", "view", "5", "--json", "title,state"],
            &["pr", "view", "--json", "5"],
        ];
        for args in views {
            let r = s.run(&b, args, &[]);
            assert_eq!(r.code, 0, "{args:?}: {}", r.err);
            assert_eq!(parsed(&r.out), Some(body.clone()), "{args:?}: {}", r.out);
        }
        finish(&b, &name);
    }
);

parity!(
    box_gh_shim_refuses_another_repo_or_owner_and_accepts_the_task_repo,
    covers = ["C-114", "REQ-39"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("guard");
        let s = shim(&b, &name);
        b.gh.route("GET", &pulls(&b), vec![Reply::ok(&json!([]))]);
        pr_reply(&b);
        let accepted: [&[&str]; 3] = [
            &["pr", "list", "--repo", "acme/widget"],
            &["pr", "list", "-R", "ACME/Widget"],
            &["pr", "list", "--repo=acme/widget"],
        ];
        for args in accepted {
            let r = s.run(&b, args, &[]);
            assert_eq!(r.code, 0, "{args:?}: {}", r.err);
        }
        let seen = sent(&b, "GET", &pulls(&b)).len();
        assert_eq!(seen, 3);
        let r = s.run(&b, &["pr", "list", "--repo", "other/repo"], &[]);
        assert_eq!(r.code, 2);
        assert!(
            r.err.contains("does not match the Task repo") && r.err.contains("acme/widget"),
            "{}",
            r.err
        );
        let evil = [
            "pr",
            "create",
            "--title",
            "t",
            "--body",
            "b",
            "--head",
            "evil:feat",
        ];
        let r = s.run(&b, &evil, &[]);
        assert_eq!(r.code, 2);
        assert!(r.err.contains("does not match the Task repo"), "{}", r.err);
        assert_eq!(
            sent(&b, "GET", &pulls(&b)).len(),
            seen,
            "refusals made no request"
        );
        assert!(sent(&b, "POST", &pulls(&b)).is_empty());

        let unattended = [
            "pr",
            "create",
            "--repo",
            "acme/widget",
            "--head",
            "acme:feat",
            "--title",
            "t",
            "--body",
            "b",
        ];
        let ok = s.run(&b, &unattended, &[]);
        assert_eq!(ok.code, 0, "{}", ok.err);
        assert_eq!(ok.out.trim(), "https://github.com/acme/widget/pull/7");
        let posted = sent(&b, "POST", &pulls(&b));
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].json()["head"], "acme:feat");
        let plain = [
            "pr", "create", "--head", "feat", "--title", "t", "--body", "b",
        ];
        let plain = s.run(&b, &plain, &[]);
        assert_eq!(plain.code, 0, "{}", plain.err);

        let moved = [("MOOR_GH_REPO_CANONICAL", "neworg/Gadget")];
        let canon = s.run(&b, &["pr", "list", "--repo", "neworg/gadget"], &moved);
        assert_eq!(canon.code, 0, "{}", canon.err);
        let head = [
            "pr",
            "create",
            "--head",
            "neworg:feat",
            "--title",
            "t",
            "--body",
            "b",
        ];
        let head = s.run(&b, &head, &moved);
        assert_eq!(head.code, 0, "{}", head.err);
        let bad = s.run(&b, &["pr", "list", "--repo", "neworg/gadget"], &[]);
        assert_eq!(bad.code, 2, "canonical name is accepted only when exported");
        finish(&b, &name);
    }
);

parity!(
    box_gh_shim_pr_create_escapes_json_and_defaults_head_and_base,
    covers = ["C-115"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("create");
        let s = shim(&b, &name);
        pr_reply(&b);
        let r = s.run(&b, &["pr", "create", "--title", "only a title"], &[]);
        assert_eq!(r.code, 2);
        assert!(r.err.contains("needs --title and --body"), "{}", r.err);
        assert!(sent(&b, "POST", &pulls(&b)).is_empty());

        let title = "say \"hi\" \\ back\tslash";
        let body = "line one\r\nline \"two\"\n\\end";
        let r = s.run(&b, &["pr", "create", "--title", title, "--body", body], &[]);
        assert_eq!(r.code, 0, "{}", r.err);
        assert_eq!(r.out, "https://github.com/acme/widget/pull/7\n");
        let j = sent(&b, "POST", &pulls(&b))[0].json();
        assert_eq!(j["title"], title);
        assert_eq!(j["body"], body);
        assert_eq!(j["head"], "feature/x");
        assert_eq!(j["base"], "main");
        assert_eq!(j["draft"], false);

        let st =
            b.w.command("git")
                .current_dir(&s.repo)
                .args([
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    "refs/remotes/origin/trunk",
                ])
                .status()
                .expect("git");
        assert!(st.success());
        let r = s.run(&b, &["pr", "create", "--title", "t", "--body", "b"], &[]);
        assert_eq!(r.code, 0, "{}", r.err);
        assert_eq!(sent(&b, "POST", &pulls(&b))[1].json()["base"], "trunk");

        let bare = json!({ "number": 9 });
        b.gh.reroute("POST", &pulls(&b), vec![Reply::json(201, &bare)]);
        let r = s.run(&b, &["pr", "create", "--title", "t", "--body", "b"], &[]);
        assert_eq!(r.code, 0, "{}", r.err);
        assert!(
            r.out.contains("\"number\"") && r.out.contains('9'),
            "raw response without html_url: {}",
            r.out
        );
        finish(&b, &name);
    }
);

parity!(
    box_gh_shim_view_list_and_comment_talk_to_the_pull_routes,
    covers = ["C-116"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("read");
        let s = shim(&b, &name);
        let one = json!({ "number": 5, "title": "Fix it", "state": "open", "html_url": "U5" });
        b.gh.route("GET", &format!("{}/5", pulls(&b)), vec![Reply::ok(&one)]);
        let r = s.run(&b, &["pr", "view", "5"], &[]);
        let want = "title: Fix it\nstate: open\nurl: U5\n";
        assert_eq!((r.code, r.out.as_str()), (0, want), "{}", r.err);
        let raw = s.run(&b, &["pr", "view", "5", "--json"], &[]);
        assert_eq!(parsed(&raw.out), Some(one.clone()));

        let rows = json!([
            { "number": 1, "title": "First", "state": "open", "html_url": "U1", "user": { "id": 99 } },
            { "number": 2, "title": "Second", "state": "open", "html_url": "U2", "user": { "id": 98 } },
        ]);
        b.gh.route("GET", &pulls(&b), vec![Reply::ok(&rows)]);
        let r = s.run(&b, &["pr", "list"], &[]);
        let want = "#1 First\n#2 Second\n";
        assert_eq!((r.code, r.out.as_str()), (0, want), "{}", r.err);
        let raw = s.run(&b, &["pr", "list", "--json", "number,title"], &[]);
        assert_eq!(parsed(&raw.out), Some(rows.clone()));

        let cur = s.run(&b, &["pr", "view"], &[]);
        let want = "title: First\nstate: open\nurl: U1\n";
        assert_eq!((cur.code, cur.out.as_str()), (0, want), "{}", cur.err);
        let urls = s.curl_urls();
        let want = format!(
            "{}/repos/acme/widget/pulls?head=acme:feature/x&state=open",
            b.gh.url()
        );
        assert!(urls.contains(&want), "{urls:?}");

        b.gh.reroute("GET", &pulls(&b), vec![Reply::ok(&json!([]))]);
        let none = s.run(&b, &["pr", "view"], &[]);
        assert_eq!(none.code, 1);
        assert!(none.err.contains("no pull request found"), "{}", none.err);

        let comments = format!("{}/issues/7/comments", b.fx.repo_path());
        let reply = Reply::json(201, &json!({ "html_url": "C7" }));
        b.gh.route("POST", &comments, vec![reply]);
        let r = s.run(&b, &["pr", "comment", "7", "--body", "looks \"good\""], &[]);
        assert_eq!((r.code, r.out.as_str()), (0, "C7\n"), "{}", r.err);
        let sent_body = sent(&b, "POST", &comments)[0].json();
        assert_eq!(sent_body, json!({ "body": "looks \"good\"" }));
        let incomplete: [&[&str]; 2] = [&["pr", "comment", "--body", "x"], &["pr", "comment", "7"]];
        for args in incomplete {
            let r = s.run(&b, args, &[]);
            assert_eq!(r.code, 2, "{args:?}: {}", r.err);
        }
        assert_eq!(sent(&b, "POST", &comments).len(), 1);
        finish(&b, &name);
    }
);

parity!(
    box_gh_shim_reports_http_failures_on_stderr_and_never_sends_credentials,
    covers = ["C-117", "C-118"],
    tags = [],
    {
        let b = bx(true);
        let name = b.w.task_name("http");
        let s = shim(&b, &name);
        b.gh.route("GET", &pulls(&b), vec![Reply::message(403, "forbidden")]);
        let r = s.run(&b, &["pr", "list"], &[]);
        assert_eq!(r.code, 1);
        assert!(
            r.err.contains("forbidden") && r.out.is_empty(),
            "{} / {}",
            r.out,
            r.err
        );
        let invalid = Reply::json(422, &json!({ "message": "Validation Failed" }));
        b.gh.route("POST", &pulls(&b), vec![invalid]);
        let r = s.run(&b, &["pr", "create", "--title", "t", "--body", "b"], &[]);
        assert_eq!(r.code, 1);
        assert!(
            r.err.contains("Validation Failed") && r.out.is_empty(),
            "{} / {}",
            r.out,
            r.err
        );

        pr_reply(&b);
        let token = "ghp_PARITY_HOST_TOKEN";
        let env = [("GH_TOKEN", token), ("GITHUB_TOKEN", token)];
        let calls: [&[&str]; 3] = [
            &["pr", "list"],
            &["pr", "create", "--title", "t", "--body", "b"],
            &["pr", "comment", "3", "--body", "c"],
        ];
        for args in calls {
            s.run(&b, args, &env);
        }
        let reqs: Vec<_> =
            b.gh.requests()
                .into_iter()
                .filter(|r| r.path.contains("/pulls") || r.path.contains("/issues"))
                .collect();
        assert!(reqs.len() >= 5, "{reqs:?}");
        for r in &reqs {
            assert!(r.path.starts_with("/repos/acme/widget/"), "{}", r.path);
            assert!(
                r.header("authorization").is_none(),
                "credential header on {}",
                r.path
            );
            let headers = format!("{:?}", r.headers);
            assert!(!headers.contains(token) && !r.body.contains(token));
        }
        let base = b.gh.url();
        let urls = s.curl_urls();
        assert!(urls.iter().all(|u| u.starts_with(&base)), "{urls:?}");
        assert!(!s.delivered.contains("GH_TOKEN") && !s.delivered.contains("Authorization"));
        finish(&b, &name);
    }
);
