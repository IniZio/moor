use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use parity::adapter::{ExitClass, Outcome, ProxyEndpoint, StartOpts, TaskView};
use parity::assert::assert_secret_absent;
use parity::fake::{claude, crabbox, herdr, Call, Rule};
use parity::parity;
use parity::world::config::RepoConfig;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

struct Fx {
    w: World,
    repo: Repo,
    name: String,
}

fn fx_repo() -> Fx {
    let f = fx();
    let keep_github =
        f.w.command("git")
            .args([
                "config",
                "--global",
                "url.https://github.com/.pushInsteadOf",
                "https://github.com/",
            ])
            .output()
            .expect("git config");
    assert!(keep_github.status.success());
    f
}

fn user_doc(extra: Value) -> Value {
    let mut doc = json!({
        "box": { "provider": "namespace-devbox" },
        "github": { "you_token_cmd": "echo ghs_fixture_token" },
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        doc[k] = v;
    }
    doc
}

fn fx() -> Fx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    let user = user_doc(json!({}));
    std::fs::create_dir_all(w.config_path().parent().expect("parent")).expect("mkdir");
    std::fs::write(w.config_path(), user.to_string()).expect("write user config");
    RepoConfig::default()
        .agent_location("box")
        .write(&repo.path);
    crabbox::install_preset(&w, crabbox::Preset::NamespaceDevbox);
    crabbox::ssh_channel(&w);
    crabbox::tunnel(&w);
    crabbox::connect(&w);
    herdr::install(&w);
    claude::install(&w);
    w.prepend_rules(
        "ssh",
        [Rule::any()
            .exec("cd '{box}' && {last}")
            .rewrite("/tmp/", "{box}/tmp/")
            .capture_stdin()],
    );
    let name = w.task_name("px");
    Fx { w, repo, name }
}

fn poll(secs: u64, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.contains(") Z"))
}

fn pgrp(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(2)?
        .parse()
        .ok()
}

fn cmdline(pid: u32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .unwrap_or_default()
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

fn listening_addrs(port: u16) -> Vec<String> {
    let want = format!("{port:04X}");
    let mut found = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let text = std::fs::read_to_string(table).unwrap_or_default();
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            let Some((addr, p)) = f.get(1).and_then(|l| l.rsplit_once(':')) else {
                continue;
            };
            if f.get(3) == Some(&"0A") && p == want {
                found.push(addr.to_string());
            }
        }
    }
    found
}

struct Resp {
    statuses: Vec<u16>,
    raw: String,
}

impl Resp {
    fn status(&self) -> Option<u16> {
        self.statuses.last().copied()
    }

    fn body(&self) -> &str {
        self.raw.rsplit_once("\r\n\r\n").map_or("", |x| x.1)
    }

    fn head_has(&self, needle: &str) -> bool {
        let head = self.raw.split("\r\n\r\n").next().unwrap_or("");
        head.to_lowercase().contains(needle)
    }
}

fn send(port: u16, raw: &[u8]) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect proxy");
    s.set_read_timeout(Some(Duration::from_secs(8))).ok();
    let _ = s.write_all(raw);
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    let raw = String::from_utf8_lossy(&buf).into_owned();
    let statuses = raw
        .split("\r\n")
        .filter_map(|l| l.strip_prefix("HTTP/1.1 "))
        .filter_map(|l| l.get(..3)?.parse().ok())
        .collect();
    Resp { statuses, raw }
}

fn call(port: u16, method: &str, target: &str, headers: &[(&str, &str)], body: &str) -> Resp {
    let mut r = format!("{method} {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (k, v) in headers {
        r.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() || !matches!(method, "GET" | "HEAD" | "DELETE") {
        r.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    r.push_str("\r\n");
    r.push_str(body);
    send(port, r.as_bytes())
}

fn via(ep: &ProxyEndpoint, method: &str, rest: &str) -> Resp {
    call(ep.port, method, &format!("/t/{}{rest}", ep.nonce), &[], "")
}

fn bearer(ep: &ProxyEndpoint, method: &str, target: &str) -> Resp {
    let auth = format!("Bearer {}", ep.nonce);
    call(ep.port, method, target, &[("Authorization", &auth)], "")
}

fn expect_all(ep: &ProxyEndpoint, status: u16, cases: &[(&str, &str)]) {
    for (method, rest) in cases {
        let r = via(ep, method, rest);
        assert_eq!(r.status(), Some(status), "{method} {rest}: {}", r.raw);
    }
}

impl Fx {
    fn start_named(&self, name: &str, goal: bool) -> Outcome<TaskView> {
        let opts = StartOpts {
            goal: goal.then(|| "do the work".to_string()),
            ..Default::default()
        };
        moor().start(&self.w, &self.repo.path, name, &opts)
    }

    fn up_named(&self, name: &str) -> ProxyEndpoint {
        let o = self.start_named(name, true);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        moor()
            .proxy_endpoint(&self.w, name)
            .expect("proxy endpoint after a Box agent launch")
    }

    fn up(&self) -> ProxyEndpoint {
        self.up_named(&self.name)
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.w
            .state_dir()
            .join("proxy")
            .join(format!("{name}.json"))
    }

    fn state(&self, name: &str) -> Value {
        let raw = std::fs::read(self.state_path(name)).expect("proxy state file");
        serde_json::from_slice(&raw).expect("state json")
    }

    fn pid(&self, name: &str) -> u32 {
        self.state(name)["pid"].as_u64().expect("state pid") as u32
    }

    fn log(&self, name: &str) -> String {
        let p = self.w.state_dir().join("proxy").join(format!("{name}.log"));
        std::fs::read_to_string(p).unwrap_or_default()
    }

    fn tunnels(&self) -> Vec<Call> {
        self.w
            .calls("crabbox")
            .into_iter()
            .filter(|c| c.argv.first().map(String::as_str) == Some("tunnel"))
            .collect()
    }

    fn user(&self, doc: Value) {
        std::fs::write(self.w.config_path(), doc.to_string()).expect("write user config");
    }

    fn close(&self, name: &str) {
        let sup = self.pid(name);
        let o = moor().close(&self.w, &self.repo.path, name, false);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(
            poll(15, || !alive(sup)),
            "supervisor {sup} alive after close"
        );
    }
}

parity!(
    proxy_listener_is_loopback_only_and_is_the_tunnel_host_port,
    covers = ["C-64", "C-88"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        assert_eq!(listening_addrs(ep.port), ["0100007F"], "loopback only");
        let t = f.tunnels();
        assert_eq!(t.len(), 1, "one tunnel");
        assert_eq!(t[0].argv[2], format!("{}:{}", ep.box_port, ep.port));
        f.close(&f.name);
    }
);

parity!(
    proxy_requires_the_task_nonce_else_401,
    covers = ["C-65", "REQ-38"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let last = if ep.nonce.ends_with('0') { '1' } else { '0' };
        let near = format!("{}{last}", &ep.nonce[..ep.nonce.len() - 1]);
        let basic = format!("Basic {}", ep.nonce);
        let bad = format!("Bearer {near}");
        let denied: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("/v1/messages", vec![]),
            ("/v1/messages", vec![("Authorization", &bad)]),
            ("/v1/messages", vec![("Authorization", "Bearer ")]),
            ("/v1/messages", vec![("Authorization", &basic)]),
            ("/v1/messages", vec![("x-api-key", &ep.nonce)]),
            ("/t//v1/messages", vec![]),
            ("/t/x/v1/messages", vec![]),
        ];
        for (target, headers) in &denied {
            let r = call(ep.port, "GET", target, headers, "");
            assert_eq!(r.status(), Some(401), "{target} {headers:?}: {}", r.raw);
            assert_eq!(r.body(), "unauthorized\n");
            assert!(r.head_has("connection: close"));
        }
        let wrong_path = call(ep.port, "GET", &format!("/t/{near}/v1/x"), &[], "");
        assert_eq!(wrong_path.status(), Some(401));
        let by_header = bearer(&ep, "GET", "/v1/models");
        assert_eq!(by_header.status(), Some(502), "{}", by_header.raw);
        let by_path = via(&ep, "GET", "/v1/models");
        assert_eq!(by_path.status(), Some(502), "{}", by_path.raw);
        let bare = call(ep.port, "GET", &format!("/t/{}", ep.nonce), &[], "");
        assert_eq!(bare.status(), Some(502), "bare /t/<nonce> maps to /");
        f.close(&f.name);
    }
);

parity!(
    proxy_routes_by_prefix_with_a_separator,
    covers = ["C-66"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        expect_all(
            &ep,
            502,
            &[
                ("GET", "/github-apix/foo"),
                ("GET", "/githubx/foo"),
                ("GET", "/v1/messages"),
            ],
        );
        expect_all(
            &ep,
            403,
            &[
                ("GET", "/github/foo"),
                ("GET", "/github?x=1"),
                ("GET", "/github-api/foo"),
                ("GET", "/github-api?x=1"),
                ("GET", "/github-api"),
            ],
        );
        let log = f.log(&f.name);
        assert!(log.contains("GET anthropic /githubx/foo 502"), "{log}");
        assert!(log.contains("GET github /foo 403"), "{log}");
        assert!(log.contains("GET github-api /foo 403"), "{log}");
        f.close(&f.name);
    }
);

parity!(
    proxy_rejects_a_target_that_does_not_start_with_slash,
    covers = ["C-67"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let r = bearer(&ep, "GET", "http://example.invalid/v1/messages");
        assert_eq!(r.status(), Some(400), "{}", r.raw);
        assert_eq!(r.body(), "bad request target\n");
        f.close(&f.name);
    }
);

parity!(
    proxy_claude_route_fails_502_when_the_token_is_unavailable,
    covers = ["C-68"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let none = via(&ep, "POST", "/v1/messages");
        assert_eq!(none.status(), Some(502));
        assert_eq!(none.body(), "credential unavailable\n");
        let missing = f.w.root().join("secrets/missing-claude");
        f.user(user_doc(json!({
            "credentials": { "CLAUDE_CODE_OAUTH_TOKEN": { "file": missing } }
        })));
        let unreadable = via(&ep, "POST", "/v1/messages");
        assert_eq!(unreadable.status(), Some(502));
        assert_eq!(unreadable.body(), none.body());
        f.close(&f.name);
    }
);

parity!(
    proxy_github_routes_deny_everything_without_a_task_repo,
    covers = ["C-70", "C-73", "REQ-38"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        expect_all(
            &ep,
            403,
            &[
                ("GET", "/github/acme/widget/info/refs"),
                ("POST", "/github/acme/widget/git-upload-pack"),
                ("GET", "/github-api/repos/acme/widget/pulls"),
                ("POST", "/github-api/repos/acme/widget/pulls"),
            ],
        );
        f.close(&f.name);
    }
);

parity!(
    proxy_log_records_requests_without_secrets,
    covers = ["C-79"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let _ = call(ep.port, "GET", "/v1/x?q=querysecret", &[], "");
        let _ = via(&ep, "GET", "/github/other/repo/info/refs?k=querysecret");
        let _ = via(&ep, "GET", "/v1/models?k=querysecret");
        let log = f.log(&f.name);
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(
            lines[0],
            format!("proxy listening on 127.0.0.1:{}", ep.port)
        );
        assert!(lines.contains(&"GET none - 401"), "{log}");
        assert!(
            lines.contains(&"GET github /other/repo/info/refs 403"),
            "{log}"
        );
        assert!(lines.contains(&"GET anthropic /v1/models 502"), "{log}");
        assert!(!log.contains(&ep.nonce) && !log.contains("querysecret"));
        let dir = f.w.state_dir().join("proxy");
        assert_eq!(mode(&dir.join(format!("{}.log", f.name))), 0o600);
        assert_nonce_host_clean(&f.w, &ep.nonce);
        f.close(&f.name);
    }
);

parity!(
    proxy_tolerates_a_bare_connect_and_close,
    covers = ["C-80"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let before = f.log(&f.name).lines().count();
        let s = TcpStream::connect(("127.0.0.1", ep.port)).expect("connect");
        s.shutdown(Shutdown::Both).ok();
        drop(s);
        let r = call(ep.port, "GET", "/v1/models", &[], "");
        assert_eq!(r.status(), Some(401));
        let log = f.log(&f.name);
        assert_eq!(
            log.lines().count(),
            before + 1,
            "probe logged nothing: {log}"
        );
        assert!(alive(f.pid(&f.name)));
        f.close(&f.name);
    }
);

parity!(
    proxy_transport_limits_and_expect_continue,
    covers = ["C-77"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let mut many: Vec<(String, String)> =
            (0..101).map(|i| (format!("X-H{i}"), "v".into())).collect();
        many.push(("Authorization".into(), format!("Bearer {}", ep.nonce)));
        let refs: Vec<(&str, &str)> = many.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let crowded = call(ep.port, "GET", "/v1/x", &refs, "");
        assert_eq!(crowded.status(), None, "more than 100 headers is dropped");
        let auth = format!("Bearer {}", ep.nonce);
        let cont = call(
            ep.port,
            "POST",
            "/v1/messages",
            &[("Authorization", &auth), ("Expect", "100-continue")],
            "{}",
        );
        assert_eq!(cont.statuses, [100, 502], "{}", cont.raw);
        f.close(&f.name);
    }
);

parity!(
    proxy_state_file_has_the_documented_shape_and_modes,
    covers = ["C-86"],
    tags = [],
    {
        let f = fx();
        let o = f.start_named(&f.name, true);
        let handle = o.ok().boxed.clone().and_then(|b| b.handle).expect("handle");
        let dir = f.w.state_dir().join("proxy");
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&f.state_path(&f.name)), 0o600);
        let st = f.state(&f.name);
        assert_eq!(st["ready"], json!(true));
        assert_eq!(st["handle"], json!(handle));
        for key in ["nonce", "box_port", "pid", "host_port", "canonical"] {
            assert!(st.get(key).is_some(), "state lacks {key}: {st}");
        }
        assert!(st["host_port"].as_u64().unwrap_or(0) > 0);
        f.close(&f.name);
        assert!(poll(10, || !f.state_path(&f.name).exists()), "state stays");
    }
);

parity!(
    proxy_nonce_is_64_hex_fresh_per_supervisor_and_never_in_argv,
    covers = ["C-84"],
    tags = [],
    {
        let f = fx();
        let first = f.up();
        let other = f.w.task_name("px2");
        let second = f.up_named(&other);
        for ep in [&first, &second] {
            assert_eq!(ep.nonce.len(), 64);
            assert!(ep
                .nonce
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        }
        assert_ne!(first.nonce, second.nonce);
        let sup = f.pid(&f.name);
        assert!(cmdline(sup).iter().all(|a| !a.contains(&first.nonce)));
        let checked = moor().proxy_check(&f.w, &f.repo.path, &f.name);
        assert!(!checked.stdout.contains(&first.nonce));
        f.close(&f.name);
        f.close(&other);
        assert_nonce_host_clean(&f.w, &first.nonce);
        assert_nonce_host_clean(&f.w, &second.nonce);
    }
);

parity!(
    proxy_ignores_a_foreign_pid_in_stale_state_and_keeps_its_port,
    covers = ["C-83"],
    tags = [],
    {
        let f = fx();
        let dir = f.w.state_dir().join("proxy");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let stale = "a".repeat(64);
        let doc = json!({
            "nonce": stale, "box_port": 31337, "handle": "stale/handle", "pid": 1,
            "ready": true, "host_port": 1, "canonical": null
        });
        let path = f.state_path(&f.name);
        std::fs::write(&path, doc.to_string()).expect("write stale state");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        let ep = f.up();
        assert_ne!(ep.nonce, stale, "a foreign pid is never reused");
        assert_eq!(ep.box_port, 31337, "a free recorded port is kept");
        assert_ne!(f.pid(&f.name), 1);
        assert_eq!(f.tunnels().len(), 1);
        f.close(&f.name);
    }
);

parity!(
    proxy_box_ports_follow_the_task_name_hash_and_probe_past_taken,
    covers = ["C-85"],
    tags = [],
    {
        let f = fx();
        let other = f.w.task_name("px2");
        let a = f.up();
        let b = f.up_named(&other);
        let span = |n: &str| 20000 + (fnv1a(n) % 20000) as u16;
        assert_ne!(a.box_port, b.box_port);
        assert_eq!(a.box_port, span(&f.name), "first task takes its hash port");
        let want = span(&other);
        let probed = if want == a.box_port { want + 1 } else { want };
        assert_eq!(b.box_port, probed);
        assert!(alive(f.pid(&f.name)) && alive(f.pid(&other)));
        f.close(&f.name);
        f.close(&other);
    }
);

parity!(
    proxy_starts_at_first_box_agent_launch_and_relaunch_reuses_it,
    covers = ["C-82", "C-83"],
    tags = [],
    {
        let f = fx();
        let idle = f.start_named(&f.name, false);
        assert_eq!(idle.class, ExitClass::Ok, "{}", idle.stderr);
        assert!(
            !f.w.state_dir().join("proxy").exists(),
            "no proxy without a goal"
        );
        assert!(f.tunnels().is_empty());
        let other = f.w.task_name("px2");
        let ep = f.up_named(&other);
        let sup = f.pid(&other);
        assert_eq!(f.tunnels().len(), 1);
        herdr::pane_gone(&f.w);
        let again = f.start_named(&other, true);
        assert_eq!(again.class, ExitClass::Ok, "{}", again.stderr);
        assert_eq!(f.tunnels().len(), 1, "relaunch reuses the tunnel");
        assert_eq!(f.pid(&other), sup);
        assert_eq!(f.state(&other)["nonce"], json!(ep.nonce));
        f.close(&other);
        f.close_plain(&f.name);
    }
);

impl Fx {
    fn close_plain(&self, name: &str) {
        let o = moor().close(&self.w, &self.repo.path, name, false);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
    }
}

parity!(
    proxy_supervisor_is_detached_and_runs_the_reverse_tunnel_in_the_worktree,
    covers = ["C-87", "C-88", "B-108", "B-111"],
    tags = [],
    {
        let f = fx();
        let o = f.start_named(&f.name, true);
        let task = o.ok().clone();
        let ep = moor().proxy_endpoint(&f.w, &f.name).expect("endpoint");
        let sup = f.pid(&f.name);
        assert_eq!(pgrp(sup), Some(sup), "own process group");
        for fd in 0..3 {
            let t = std::fs::read_link(format!("/proc/{sup}/fd/{fd}"));
            assert_eq!(t.ok(), Some(PathBuf::from("/dev/null")), "fd {fd}");
        }
        let args = cmdline(sup);
        let has = |flag: &str, val: &str| args.windows(2).any(|p| p[0] == flag && p[1] == val);
        assert!(args.iter().any(|a| a == "proxy"), "{args:?}");
        assert!(args.iter().any(|a| a == "serve"), "{args:?}");
        assert!(has("--box-port", &ep.box_port.to_string()), "{args:?}");
        let handle = task.boxed.clone().and_then(|b| b.handle).expect("handle");
        assert!(has("--handle", &handle), "{args:?}");
        let tunnel = &f.tunnels()[0];
        let box_name = handle.split_once('/').expect("provider/name").1;
        let want: Vec<String> = [
            "tunnel",
            "--reverse",
            &format!("{}:{}", ep.box_port, ep.port),
            "--id",
            box_name,
            "--json",
            "--provider",
            "namespace-devbox",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(tunnel.argv, want);
        assert_eq!(Some(tunnel.cwd.as_str()), task.worktree.as_deref());
        f.close(&f.name);
    }
);

parity!(
    proxy_close_stops_supervisor_tunnel_and_listener,
    covers = ["C-90"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let sup = f.pid(&f.name);
        let tunnel = f.tunnels()[0].pid;
        assert!(alive(sup) && alive(tunnel));
        f.close(&f.name);
        assert!(
            poll(10, || !alive(tunnel)),
            "tunnel {tunnel} outlived close"
        );
        let addr = format!("127.0.0.1:{}", ep.port).parse().expect("addr");
        let refused = TcpStream::connect_timeout(&addr, Duration::from_secs(1));
        assert!(refused.is_err(), "listener still open");
        assert!(poll(10, || !f.state_path(&f.name).exists()), "state stays");
    }
);

parity!(
    proxy_restarts_a_tunnel_that_exits,
    covers = ["C-89", "B-110", "REQ-38"],
    tags = [],
    {
        let f = fx();
        f.w.prepend_rules(
            "crabbox",
            [Rule::on(&["tunnel"])
                .times(1)
                .stdout("{\"box\":20000,\"host\":20000}\n")],
        );
        let ep = f.up();
        let sup = f.pid(&f.name);
        assert!(poll(15, || f.tunnels().len() >= 2), "no second tunnel");
        assert_eq!(f.pid(&f.name), sup, "same supervisor");
        assert_eq!(f.state(&f.name)["ready"], json!(true));
        assert_eq!(f.state(&f.name)["nonce"], json!(ep.nonce));
        f.close(&f.name);
    }
);

parity!(
    proxy_check_reports_a_dead_supervisor_without_a_box_request,
    covers = ["C-93", "C-94"],
    tags = [],
    {
        let f = fx();
        f.up();
        let sup = f.pid(&f.name);
        unsafe { libc::kill(-(sup as i32), libc::SIGKILL) };
        assert!(poll(10, || !alive(sup)));
        let ssh_before = f.w.calls("ssh").len();
        let o = moor().proxy_check(&f.w, &f.repo.path, &f.name);
        assert_eq!(o.class, ExitClass::Failed, "{}", o.stdout);
        let c = o.value.as_ref().expect("report on stdout");
        assert!(!c.supervisor_alive);
        assert_eq!(c.tunnel, "unknown");
        assert_eq!(c.box_status, None);
        assert!(c.box_port.is_some() && c.host_port.is_some());
        assert_eq!(f.w.calls("ssh").len(), ssh_before, "no Box request");
        let missing = moor().proxy_check(&f.w, &f.repo.path, "no-such-task");
        assert_eq!(missing.class, ExitClass::Failed);
        f.close_plain(&f.name);
    }
);

fn relay(listen: u16, target: u16) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", listen)).expect("bind box port");
    listener.set_nonblocking(true).expect("nonblocking");
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            let Ok((a, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            a.set_nonblocking(false).ok();
            let Ok(b) = TcpStream::connect(("127.0.0.1", target)) else {
                continue;
            };
            let pairs = [
                (a.try_clone(), b.try_clone()),
                (b.try_clone(), a.try_clone()),
            ];
            for (from, to) in pairs {
                let (Ok(mut from), Ok(mut to)) = (from, to) else {
                    continue;
                };
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut from, &mut to);
                    let _ = to.shutdown(Shutdown::Write);
                });
            }
        }
    });
    (stop, handle)
}

fn assert_nonce_host_clean(w: &World, nonce: &str) {
    for dir in cred_dirs(w) {
        let _ = std::fs::remove_dir_all(w.box_root().join("tmp").join(dir));
    }
    assert_secret_absent(w, nonce, &["proxy/"]);
}

fn cred_dirs(w: &World) -> Vec<String> {
    std::fs::read_dir(w.box_root().join("tmp"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("moor-cred-"))
                .collect()
        })
        .unwrap_or_default()
}

parity!(
    proxy_check_probes_through_the_tunnel_with_status_classes,
    covers = ["C-93", "C-94", "C-95", "C-96"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let dirs_before = cred_dirs(&f.w);
        let down = moor().proxy_check(&f.w, &f.repo.path, &f.name);
        assert_eq!(down.class, ExitClass::Failed);
        let dc = down.value.as_ref().expect("report on stdout");
        assert!(dc.supervisor_alive);
        assert_eq!((dc.tunnel.as_str(), dc.box_status), ("down", Some(0)));
        let (stop, handle) = relay(ep.box_port, ep.port);
        let up = moor().proxy_check(&f.w, &f.repo.path, &f.name);
        assert_eq!(up.class, ExitClass::Ok, "{}{}", up.stdout, up.stderr);
        let c = up.ok();
        assert_eq!((c.tunnel.as_str(), c.box_status), ("up", Some(403)));
        assert_eq!(
            (c.box_port, c.host_port),
            (Some(ep.box_port), Some(ep.port))
        );
        stop.store(true, Ordering::Relaxed);
        handle.join().expect("relay");
        assert_eq!(cred_dirs(&f.w), dirs_before, "probe dir removed");
        assert_nonce_host_clean(&f.w, &ep.nonce);
        f.close(&f.name);
    }
);

fn needs_backoff_clock() {
    assert!(
        moor().caps().contains(&"ProxyBackoffClock"),
        "adapter cannot shrink the supervisor backoff"
    );
}

parity!(
    proxy_that_never_becomes_ready_gives_up_after_five_attempts,
    covers = ["B-109"],
    tags = ["new_only(ProxyBackoffClock)"],
    {
        needs_backoff_clock();
        let f = fx();
        std::fs::write(
            f.w.root().join("proxy-backoff.json"),
            json!({ "ms": 20 }).to_string(),
        )
        .expect("write proxy-backoff.json");
        f.w.prepend_rules(
            "crabbox",
            [Rule::on(&["tunnel"]).stdout("not json\n").block()],
        );
        let _ = f.start_named(&f.name, true);
        let gave_up = poll(2, || !f.state_path(&f.name).exists());
        assert!(gave_up, "supervisor never gave up");
        assert_eq!(f.tunnels().len(), 5, "five unready attempts");
        let ran =
            f.w.calls("herdr")
                .into_iter()
                .filter(|c| c.argv.starts_with(&["pane".to_string(), "run".to_string()]))
                .count();
        assert_eq!(ran, 0, "agent pane started without a proxy");
        f.close_plain(&f.name);
    }
);

parity!(
    anthropic_route_has_no_path_or_method_allow_list,
    covers = ["C-69"],
    tags = [],
    {
        let f = fx();
        let ep = f.up();
        let cases = [
            ("GET", "/"),
            ("GET", "/v1/messages"),
            ("POST", "/v1/messages"),
            ("DELETE", "/v1/organizations/o1/api_keys/k1"),
            ("PUT", "/admin/anything"),
            ("PATCH", "/v1/oauth/token"),
            ("GET", "/api/oauth/profile"),
        ];
        expect_all(&ep, 502, &cases);
        let log = f.log(&f.name);
        for (m, p) in cases {
            assert!(
                log.contains(&format!("{m} anthropic {p} 502")),
                "{m} {p}: {log}"
            );
        }
        f.close(&f.name);
    }
);

parity!(
    hidden_proxy_serve_refuses_a_closed_task_and_a_missing_state,
    covers = ["C-91"],
    tags = [],
    {
        let f = fx();
        let started = f.start_named(&f.name, false);
        assert_eq!(started.class, ExitClass::Ok, "{}", started.stderr);
        let wt = started
            .value
            .and_then(|t| t.worktree)
            .expect("task worktree");
        let serve = |wt: &str| {
            moor().github_raw(
                &f.w,
                &f.repo.path,
                &[
                    "proxy",
                    "serve",
                    "--worktree",
                    wt,
                    "--handle",
                    "h",
                    "--box-port",
                    "1",
                ],
            )
        };
        let open = serve(&wt);
        assert_eq!(open.class, ExitClass::Failed, "{}", open.stderr);
        assert!(
            open.stderr
                .contains(&format!("no proxy state for task {}", f.name)),
            "{}",
            open.stderr
        );
        f.close_plain(&f.name);
        let closed = serve(&wt);
        assert_eq!(closed.class, ExitClass::Failed, "{}", closed.stderr);
        assert!(
            closed
                .stderr
                .contains(&format!("no open task for worktree {wt}")),
            "{}",
            closed.stderr
        );
        assert!(!f.state_path(&f.name).exists());
    }
);

type Handler = Box<dyn Fn(&Seen, &mut TcpStream) + Send + Sync>;

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

struct Upstream {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn reply(out: &mut TcpStream, status: u16, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(body.as_bytes());
}

fn read_request(s: &mut TcpStream) -> Option<Seen> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut byte).ok()?;
        buf.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target) = (first.next()?.to_string(), first.next()?.to_string());
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let len = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).ok()?;
    let body = String::from_utf8_lossy(&body).into_owned();
    Some(Seen {
        method,
        target,
        headers,
        body,
    })
}

fn upstream(handler: Handler) -> Upstream {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind upstream");
    listener.set_nonblocking(true).expect("nonblocking");
    let port = listener.local_addr().expect("addr").port();
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let (log, flag) = (Arc::clone(&seen), Arc::clone(&stop));
    let handler = Arc::new(handler);
    std::thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            let Ok((mut s, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            s.set_nonblocking(false).ok();
            let (log, handler) = (Arc::clone(&log), Arc::clone(&handler));
            std::thread::spawn(move || {
                while let Some(req) = read_request(&mut s) {
                    log.lock().unwrap().push(req.clone());
                    handler(&req, &mut s);
                }
            });
        }
    });
    Upstream { port, seen, stop }
}

fn echo_ok() -> Handler {
    Box::new(|_, out| reply(out, 200, "{\"ok\":true}"))
}

fn use_upstream(f: &Fx, up: &Upstream) {
    let base = format!("http://127.0.0.1:{}", up.port);
    let doc = json!({ "anthropic": base, "github": base, "github_api": base });
    std::fs::write(f.w.root().join("proxy-upstream.json"), doc.to_string())
        .expect("write proxy-upstream.json");
}

fn needs_upstream() {
    assert!(
        moor().caps().contains(&"ProxyUpstream"),
        "adapter cannot point the proxy at a fake upstream"
    );
}

fn secret_file(f: &Fx, name: &str, body: &str) -> String {
    let path = f.w.root().join("secrets").join(name);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, body).expect("write secret");
    path.to_string_lossy().into_owned()
}

fn token_cmd(f: &Fx, token: &str) -> (String, PathBuf) {
    let counter = f.w.root().join("mint-count");
    (
        format!("echo x >> '{}'; echo {token}", counter.display()),
        counter,
    )
}

fn user_with_token_cmd(f: &Fx, cmd: &str) {
    f.user(user_doc(json!({ "github": { "you_token_cmd": cmd } })));
}

fn claude_user(f: &Fx, file: &str) {
    f.user(user_doc(json!({
        "credentials": { "CLAUDE_CODE_OAUTH_TOKEN": { "file": file } },
    })));
}

parity!(
    proxy_claude_route_swaps_auth_for_the_real_token,
    covers = ["C-68"],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx();
        let up = upstream(echo_ok());
        use_upstream(&f, &up);
        let real = "real-claude-token-1";
        let file = secret_file(&f, "claude", real);
        claude_user(&f, &file);
        let ep = f.up();
        let auth = format!("Bearer {}", ep.nonce);
        let r = call(
            ep.port,
            "POST",
            "/v1/messages?beta=1",
            &[
                ("Authorization", &auth),
                ("x-api-key", "placeholder"),
                ("anthropic-beta", "b1"),
                ("anthropic-version", "2023-06-01"),
                ("Content-Type", "application/json"),
            ],
            "{\"a\":1}",
        );
        assert_eq!(r.status(), Some(200), "{}", r.raw);
        let seen = up.seen.lock().unwrap().clone();
        let s = &seen[0];
        assert_eq!(
            (s.method.as_str(), s.target.as_str()),
            ("POST", "/v1/messages?beta=1")
        );
        assert_eq!(
            s.header("authorization"),
            Some("Bearer real-claude-token-1")
        );
        assert_eq!(s.header("x-api-key"), None);
        assert_eq!(s.header("anthropic-beta"), Some("b1"));
        assert_eq!(s.header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(s.body, "{\"a\":1}");
        let by_path = call(
            ep.port,
            "GET",
            &format!("/t/{}/v1/models", ep.nonce),
            &[("Authorization", "Bearer placeholder")],
            "",
        );
        assert_eq!(by_path.status(), Some(200));
        std::fs::write(&file, "real-claude-token-2").expect("rotate token");
        let _ = via(&ep, "GET", "/v1/models");
        let seen = up.seen.lock().unwrap().clone();
        assert_eq!(seen[1].target, "/v1/models");
        assert_eq!(
            seen[2].header("authorization"),
            Some("Bearer real-claude-token-2")
        );
        for s in &seen {
            assert!(s.headers.iter().all(|(_, v)| !v.contains(&ep.nonce)));
        }
        assert_secret_absent(&f.w, real, &[]);
        f.close(&f.name);
    }
);

parity!(
    proxy_git_route_injects_the_minted_token_and_caches_it,
    covers = ["C-71", "C-72"],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx_repo();
        let up = upstream(echo_ok());
        use_upstream(&f, &up);
        let (cmd, counter) = token_cmd(&f, "ghs_minted_1");
        user_with_token_cmd(&f, &cmd);
        let ep = f.up();
        let auth = format!("Bearer {}", ep.nonce);
        let target = format!(
            "/t/{}/github/acme/widget/info/refs?service=git-upload-pack",
            ep.nonce
        );
        for _ in 0..2 {
            let r = call(ep.port, "GET", &target, &[("Authorization", &auth)], "");
            assert_eq!(r.status(), Some(200), "{}", r.raw);
        }
        let seen = up.seen.lock().unwrap().clone();
        let basic = "Basic eC1hY2Nlc3MtdG9rZW46Z2hzX21pbnRlZF8x";
        assert_eq!(seen[0].header("authorization"), Some(basic));
        assert_eq!(
            seen[0].target,
            "/acme/widget/info/refs?service=git-upload-pack"
        );
        let mints = std::fs::read_to_string(&counter)
            .unwrap_or_default()
            .lines()
            .count();
        assert_eq!(mints, 1, "token cached between requests");
        assert_secret_absent(&f.w, "ghs_minted_1", &[]);
        f.close(&f.name);
    }
);

parity!(
    proxy_rest_route_forwards_with_token_and_default_headers,
    covers = ["C-73", "C-75"],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx_repo();
        let up = upstream(echo_ok());
        use_upstream(&f, &up);
        let (cmd, _) = token_cmd(&f, "ghs_rest_1");
        user_with_token_cmd(&f, &cmd);
        let ep = f.up();
        let r = via(&ep, "GET", "/github-api/repos/acme/widget/pulls?state=open");
        assert_eq!(r.status(), Some(200), "{}", r.raw);
        let kept = call(
            ep.port,
            "GET",
            &format!("/t/{}/github-api/repos/acme/widget/pulls/3", ep.nonce),
            &[("Accept", "text/plain"), ("User-Agent", "agent/1")],
            "",
        );
        assert_eq!(kept.status(), Some(200));
        let seen = up.seen.lock().unwrap().clone();
        assert_eq!(seen[0].target, "/repos/acme/widget/pulls?state=open");
        assert_eq!(seen[0].header("authorization"), Some("token ghs_rest_1"));
        assert_eq!(
            seen[0].header("accept"),
            Some("application/vnd.github+json")
        );
        assert_eq!(seen[0].header("user-agent"), Some("moor-proxy"));
        assert_eq!(seen[1].header("accept"), Some("text/plain"));
        assert_eq!(seen[1].header("user-agent"), Some("agent/1"));
        f.close(&f.name);
    }
);

parity!(
    proxy_comment_gate_requires_a_pull_request,
    covers = ["C-74"],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx_repo();
        let up = upstream(Box::new(|req, out| {
            let status = if req.target.ends_with("/pulls/7") {
                404
            } else {
                200
            };
            reply(out, status, "{}");
        }));
        use_upstream(&f, &up);
        let (cmd, _) = token_cmd(&f, "ghs_gate_1");
        user_with_token_cmd(&f, &cmd);
        let ep = f.up();
        let plain = via(
            &ep,
            "POST",
            "/github-api/repos/acme/widget/issues/7/comments",
        );
        assert_eq!(plain.status(), Some(403), "{}", plain.raw);
        assert_eq!(
            up.seen.lock().unwrap().len(),
            1,
            "only the pull lookup was sent"
        );
        let pull = via(
            &ep,
            "POST",
            "/github-api/repos/acme/widget/issues/8/comments",
        );
        assert_eq!(pull.status(), Some(200), "{}", pull.raw);
        let seen = up.seen.lock().unwrap().clone();
        let tail: Vec<&str> = seen[1..].iter().map(|s| s.target.as_str()).collect();
        assert_eq!(
            tail,
            [
                "/repos/acme/widget/pulls/8",
                "/repos/acme/widget/issues/8/comments"
            ]
        );
        f.close(&f.name);
    }
);

parity!(
    proxy_rewrites_the_origin_name_to_the_canonical_repo,
    covers = ["C-76", "REQ-38"],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx_repo();
        let up = upstream(Box::new(|req, out| {
            if req.target == "/repos/acme/widget" {
                let doc = "{\"name\":\"gadget\",\"id\":5,\"owner\":{\"login\":\"acme\",\"id\":9}}";
                reply(out, 200, doc);
            } else {
                reply(out, 200, "{}");
            }
        }));
        use_upstream(&f, &up);
        let (cmd, _) = token_cmd(&f, "ghs_canon_1");
        user_with_token_cmd(&f, &cmd);
        let ep = f.up();
        let r = via(&ep, "GET", "/github-api/repos/acme/widget/pulls");
        assert_eq!(r.status(), Some(200), "{}", r.raw);
        let seen = up.seen.lock().unwrap().clone();
        assert_eq!(
            seen.last().map(|s| s.target.as_str()),
            Some("/repos/acme/gadget/pulls")
        );
        assert_eq!(f.state(&f.name)["canonical"], json!("acme/gadget"));
        f.close(&f.name);
    }
);

parity!(
    proxy_relays_streams_and_does_not_follow_redirects,
    covers = ["C-77", "C-78"],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx();
        let up = upstream(Box::new(|req, out| {
            if req.target.starts_with("/redirect") {
                let head = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/x\r\nContent-Length: 0\r\n\r\n";
                let _ = out.write_all(head.as_bytes());
                return;
            }
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            let _ = out.write_all(head.as_bytes());
            for part in ["data: one\n\n", "data: two\n\n"] {
                let _ = write!(out, "{:x}\r\n{part}\r\n", part.len());
                let _ = out.flush();
                std::thread::sleep(Duration::from_millis(300));
            }
            let _ = out.write_all(b"0\r\n\r\n");
        }));
        use_upstream(&f, &up);
        let file = secret_file(&f, "claude", "real-claude-token-3");
        claude_user(&f, &file);
        let ep = f.up();
        let auth = format!("Bearer {}", ep.nonce);
        let mut s = TcpStream::connect(("127.0.0.1", ep.port)).expect("connect");
        let req = format!(
            "GET /v1/stream HTTP/1.1\r\nHost: x\r\nAuthorization: {auth}\r\nConnection: close\r\n\r\n"
        );
        s.write_all(req.as_bytes()).expect("send");
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut first = [0u8; 4096];
        let n = s.read(&mut first).expect("first read");
        let early = String::from_utf8_lossy(&first[..n]).into_owned();
        assert!(early.contains("data: one"), "first event delayed: {early}");
        assert!(
            !early.contains("data: two"),
            "events were buffered together"
        );
        let mut rest = String::new();
        let _ = s.read_to_string(&mut rest);
        assert!(rest.contains("data: two"), "{rest}");
        let moved = call(ep.port, "GET", "/redirect", &[("Authorization", &auth)], "");
        assert_eq!(moved.status(), Some(302), "{}", moved.raw);
        f.close(&f.name);
    }
);

parity!(
    proxy_config_error_after_start_is_not_confused_with_missing_token,
    covers = [],
    tags = ["new_only(ProxyUpstream)"],
    {
        needs_upstream();
        let f = fx();
        let up = upstream(echo_ok());
        use_upstream(&f, &up);
        let ep = f.up();
        let missing = via(&ep, "GET", "/v1/models");
        std::fs::write(f.w.config_path(), "box: [unclosed\n  : :").expect("break config");
        let broken = via(&ep, "GET", "/v1/models");
        assert_eq!(missing.status(), Some(502));
        assert_eq!(broken.status(), Some(502));
        assert_ne!(
            missing.body(),
            broken.body(),
            "a config error must read differently from 'token not configured'"
        );
        f.close(&f.name);
    }
);

fn repo_fx(up_handler: Handler) -> (Fx, Upstream) {
    needs_upstream();
    let f = fx_repo();
    let up = upstream(up_handler);
    use_upstream(&f, &up);
    let (cmd, _) = token_cmd(&f, "ghs_repo_1");
    user_with_token_cmd(&f, &cmd);
    (f, up)
}

parity!(
    proxy_git_route_is_scoped_to_the_task_repo_and_git_endpoints,
    covers = ["C-70", "C-71"],
    tags = ["new_only(ProxyUpstream)"],
    {
        let (f, up) = repo_fx(echo_ok());
        let ep = f.up();
        let sup = cmdline(f.pid(&f.name));
        assert!(sup
            .windows(2)
            .any(|p| p[0] == "--repo" && p[1] == "acme/widget"));
        let allowed = [
            (
                "GET",
                "/github/acme/widget/info/refs?service=git-upload-pack",
            ),
            ("HEAD", "/github/acme/widget/info/refs"),
            ("POST", "/github/acme/widget.git/git-upload-pack"),
            ("POST", "/github/ACME/Widget/git-receive-pack"),
        ];
        expect_all(&ep, 200, &allowed);
        assert_eq!(up.seen.lock().unwrap().len(), allowed.len());
        let denied = [
            ("GET", "/github/acme/other/info/refs"),
            ("GET", "/github/other/widget/info/refs"),
            ("GET", "/github/acme/widget/archive/main.tar.gz"),
            ("GET", "/github/acme/widget/info/refs/x"),
            ("GET", "/github/acme/widget/../other/info/refs"),
            ("GET", "/github/acme/widget/%2e%2e/info/refs"),
            ("GET", "/github/acme/widget.git.git/info/refs"),
            ("POST", "/github/acme/widget/info/refs"),
            ("GET", "/github/acme/widget/git-upload-pack"),
            ("GET", "/github/acme/widget/git-receive-pack"),
            ("PUT", "/github/acme/widget/git-receive-pack"),
            ("GET", "/github/api/v3/repos/acme/widget"),
            ("GET", "/github/"),
            ("GET", "/github"),
        ];
        expect_all(&ep, 403, &denied);
        assert_eq!(
            up.seen.lock().unwrap().len(),
            allowed.len(),
            "denied reached upstream"
        );
        f.close(&f.name);
    }
);

parity!(
    proxy_rest_route_allows_only_pull_endpoints_of_the_task_repo,
    covers = ["C-73"],
    tags = ["new_only(ProxyUpstream)"],
    {
        let (f, up) = repo_fx(echo_ok());
        let ep = f.up();
        let r = "/github-api/repos/acme/widget";
        let allowed = [
            ("GET", format!("{r}/pulls?state=open")),
            ("POST", format!("{r}/pulls")),
            ("GET", format!("{r}/pulls/12")),
            ("PATCH", format!("{r}/pulls/12")),
            ("GET", "/github-api/repos/ACME/Widget/pulls".to_string()),
        ];
        for (m, p) in &allowed {
            let got = via(&ep, m, p);
            assert_eq!(got.status(), Some(200), "{m} {p}: {}", got.raw);
        }
        let hits = up.seen.lock().unwrap().len();
        let denied = [
            ("GET", "/github-api/repos/acme/other/pulls".to_string()),
            ("GET", "/github-api/repos/other/widget/pulls".to_string()),
            ("GET", "/github-api/user".to_string()),
            ("GET", r.to_string()),
            ("GET", format!("{r}/contents/README.md")),
            ("PUT", format!("{r}/pulls/12/merge")),
            ("GET", format!("{r}/pulls/12/files")),
            ("GET", format!("{r}/pulls/abc")),
            ("DELETE", format!("{r}/pulls/12")),
            ("PUT", format!("{r}/pulls/12")),
            ("PATCH", format!("{r}/pulls")),
            ("POST", format!("{r}/pulls/12")),
            ("PATCH", format!("{r}/issues/12/comments")),
            ("GET", format!("{r}/issues/12")),
            ("GET", format!("{r}/pulls/../issues/12")),
            ("GET", format!("{r}/pulls/./12")),
            ("GET", format!("{r}/pulls/%31")),
            ("GET", format!("{r}/pulls/1\\2")),
            ("GET", format!("{r}/pulls//12")),
            ("GET", format!("{r}/pulls/")),
        ];
        for (m, p) in &denied {
            let got = via(&ep, m, p);
            assert_eq!(got.status(), Some(403), "{m} {p}: {}", got.raw);
            assert_eq!(got.body(), "forbidden\n");
        }
        assert_eq!(
            up.seen.lock().unwrap().len(),
            hits,
            "denied reached upstream"
        );
        f.close(&f.name);
    }
);

parity!(
    proxy_mint_failure_is_502_and_closes_the_connection,
    covers = ["C-72"],
    tags = ["new_only(ProxyUpstream)"],
    {
        let (f, up) = repo_fx(echo_ok());
        user_with_token_cmd(&f, "false");
        let ep = f.up();
        let r = via(&ep, "GET", "/github/acme/widget/info/refs");
        assert_eq!(r.status(), Some(502), "{}", r.raw);
        assert_eq!(r.body(), "credential unavailable\n");
        assert!(r.head_has("connection: close"));
        assert!(up.seen.lock().unwrap().is_empty());
        f.close(&f.name);
    }
);
