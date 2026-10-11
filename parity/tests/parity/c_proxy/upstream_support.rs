use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use crate::moor;

use super::support::*;

pub(super) type Handler = Box<dyn Fn(&Seen, &mut TcpStream) + Send + Sync>;

#[derive(Clone, Debug)]
pub(super) struct Seen {
    pub(super) method: String,
    pub(super) target: String,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: String,
}

impl Seen {
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub(super) struct Upstream {
    pub(super) port: u16,
    pub(super) seen: Arc<Mutex<Vec<Seen>>>,
    pub(super) stop: Arc<AtomicBool>,
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub(super) fn reply(out: &mut TcpStream, status: u16, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(body.as_bytes());
}

pub(super) fn read_request(s: &mut TcpStream) -> Option<Seen> {
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

pub(super) fn upstream(handler: Handler) -> Upstream {
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

pub(super) fn echo_ok() -> Handler {
    Box::new(|_, out| reply(out, 200, "{\"ok\":true}"))
}

pub(super) fn use_upstream(f: &Fx, up: &Upstream) {
    let base = format!("http://127.0.0.1:{}", up.port);
    let doc = json!({ "anthropic": base, "github": base, "github_api": base });
    std::fs::write(f.w.root().join("proxy-upstream.json"), doc.to_string())
        .expect("write proxy-upstream.json");
}

pub(super) fn needs_upstream() {
    assert!(
        moor().caps().contains(&"ProxyUpstream"),
        "adapter cannot point the proxy at a fake upstream"
    );
}

pub(super) fn secret_file(f: &Fx, name: &str, body: &str) -> String {
    let path = f.w.root().join("secrets").join(name);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, body).expect("write secret");
    path.to_string_lossy().into_owned()
}

pub(super) fn token_cmd(f: &Fx, token: &str) -> (String, PathBuf) {
    f.w.register_secret(token);
    let counter = f.w.root().join("mint-count");
    (
        format!("echo x >> '{}'; echo {token}", counter.display()),
        counter,
    )
}

pub(super) fn user_with_token_cmd(f: &Fx, cmd: &str) {
    f.user(user_doc(json!({ "github": { "you_token_cmd": cmd } })));
}

pub(super) fn claude_user(f: &Fx, file: &str) {
    f.user(user_doc(json!({
        "credentials": { "CLAUDE_CODE_OAUTH_TOKEN": { "file": file } },
    })));
}

pub(super) fn repo_fx(up_handler: Handler) -> (Fx, Upstream) {
    needs_upstream();
    let f = fx_repo();
    let up = upstream(up_handler);
    use_upstream(&f, &up);
    let (cmd, _) = token_cmd(&f, "ghs_repo_1");
    user_with_token_cmd(&f, &cmd);
    (f, up)
}
