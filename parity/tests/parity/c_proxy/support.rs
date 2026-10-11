use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parity::adapter::{ExitClass, Outcome, ProxyEndpoint, StartOpts, TaskView};
use parity::assert::assert_secret_absent;
use parity::fake::{claude, crabbox, herdr, Call, Rule};

use parity::world::config::RepoConfig;
use parity::world::repo::Repo;
use parity::world::World;
use serde_json::{json, Value};

use crate::{moor, world};

pub(super) struct Fx {
    pub(super) w: World,
    pub(super) repo: Repo,
    pub(super) name: String,
}

pub(super) fn fx_repo() -> Fx {
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

pub(super) fn user_doc(extra: Value) -> Value {
    let mut doc = json!({
        "box": { "provider": "namespace-devbox" },
        "github": { "you_token_cmd": "echo ghs_fixture_token" },
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        doc[k] = v;
    }
    doc
}

pub(super) fn fx() -> Fx {
    let w = world();
    let repo = Repo::create(&w, "widget");
    let user = user_doc(json!({}));
    std::fs::create_dir_all(w.config_path().parent().expect("parent")).expect("mkdir");
    std::fs::write(w.config_path(), user.to_string()).expect("write user config");
    RepoConfig::default().write(&repo.path);
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

pub(super) fn poll(secs: u64, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

pub(super) fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.contains(") Z"))
}

pub(super) fn pgrp(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(2)?
        .parse()
        .ok()
}

pub(super) fn cmdline(pid: u32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .unwrap_or_default()
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

pub(super) fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

pub(super) fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

pub(super) fn listening_addrs(port: u16) -> Vec<String> {
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

pub(super) struct Resp {
    pub(super) statuses: Vec<u16>,
    pub(super) raw: String,
}

impl Resp {
    pub(super) fn status(&self) -> Option<u16> {
        self.statuses.last().copied()
    }

    pub(super) fn body(&self) -> &str {
        self.raw.rsplit_once("\r\n\r\n").map_or("", |x| x.1)
    }

    pub(super) fn head_has(&self, needle: &str) -> bool {
        let head = self.raw.split("\r\n\r\n").next().unwrap_or("");
        head.to_lowercase().contains(needle)
    }
}

pub(super) fn send(port: u16, raw: &[u8]) -> Resp {
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

pub(super) fn call(
    port: u16,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Resp {
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

pub(super) fn via(ep: &ProxyEndpoint, method: &str, rest: &str) -> Resp {
    call(ep.port, method, &format!("/t/{}{rest}", ep.nonce), &[], "")
}

pub(super) fn bearer(ep: &ProxyEndpoint, method: &str, target: &str) -> Resp {
    let auth = format!("Bearer {}", ep.nonce);
    call(ep.port, method, target, &[("Authorization", &auth)], "")
}

pub(super) fn expect_all(ep: &ProxyEndpoint, status: u16, cases: &[(&str, &str)]) {
    for (method, rest) in cases {
        let r = via(ep, method, rest);
        assert_eq!(r.status(), Some(status), "{method} {rest}: {}", r.raw);
    }
}

impl Fx {
    pub(super) fn start_named(&self, name: &str, goal: bool) -> Outcome<TaskView> {
        let opts = StartOpts {
            goal: goal.then(|| "do the work".to_string()),
            ..Default::default()
        };
        moor().start(&self.w, &self.repo.path, name, &opts)
    }

    pub(super) fn up_named(&self, name: &str) -> ProxyEndpoint {
        let o = self.start_named(name, true);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        moor()
            .proxy_endpoint(&self.w, name)
            .expect("proxy endpoint after a Box agent launch")
    }

    pub(super) fn up(&self) -> ProxyEndpoint {
        self.up_named(&self.name)
    }

    pub(super) fn state_path(&self, name: &str) -> PathBuf {
        self.w
            .state_dir()
            .join("proxy")
            .join(format!("{name}.json"))
    }

    pub(super) fn state(&self, name: &str) -> Value {
        let raw = std::fs::read(self.state_path(name)).expect("proxy state file");
        serde_json::from_slice(&raw).expect("state json")
    }

    pub(super) fn pid(&self, name: &str) -> u32 {
        self.state(name)["pid"].as_u64().expect("state pid") as u32
    }

    pub(super) fn log(&self, name: &str) -> String {
        let p = self.w.state_dir().join("proxy").join(format!("{name}.log"));
        std::fs::read_to_string(p).unwrap_or_default()
    }

    pub(super) fn tunnels(&self) -> Vec<Call> {
        self.w
            .calls("crabbox")
            .into_iter()
            .filter(|c| c.argv.first().map(String::as_str) == Some("tunnel"))
            .collect()
    }

    pub(super) fn user(&self, doc: Value) {
        std::fs::write(self.w.config_path(), doc.to_string()).expect("write user config");
    }

    pub(super) fn close(&self, name: &str) {
        let sup = self.pid(name);
        let o = moor().close(&self.w, &self.repo.path, name, false);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
        assert!(
            poll(15, || !alive(sup)),
            "supervisor {sup} alive after close"
        );
    }
}

impl Fx {
    pub(super) fn close_plain(&self, name: &str) {
        let o = moor().close(&self.w, &self.repo.path, name, false);
        assert_eq!(o.class, ExitClass::Ok, "{}", o.stderr);
    }
}

pub(super) fn relay(listen: u16, target: u16) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
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

pub(super) fn assert_nonce_host_clean(w: &World, nonce: &str) {
    for dir in cred_dirs(w) {
        let _ = std::fs::remove_dir_all(w.box_root().join("tmp").join(dir));
    }
    w.register_secret(nonce);
    assert_secret_absent(w, nonce, &["proxy/"]);
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
