use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};

use crate::world::World;

mod test_key;
mod wire;

pub use test_key::TEST_KEY_PEM;
pub use wire::{base64url_decode, Reply, Request};

struct Route {
    method: String,
    path: String,
    body_has: Option<String>,
    replies: Vec<Reply>,
    hits: usize,
}

impl Route {
    fn matches(&self, req: &Request, wild: bool) -> bool {
        let path_ok = match self.path.strip_suffix('*') {
            Some(prefix) => wild && req.path.starts_with(prefix),
            None => !wild && self.path == req.path,
        };
        path_ok
            && self.method == req.method
            && self
                .body_has
                .as_deref()
                .is_none_or(|needle| req.body.contains(needle))
    }
}

struct Shared {
    routes: Mutex<Vec<Route>>,
    log: Mutex<Vec<Request>>,
    stop: AtomicBool,
}

/// Fixture ids and tokens for one repo; `FakeGithub::script_repo` turns it into routes.
#[derive(Debug, Clone)]
pub struct RepoFixture {
    pub owner: String,
    pub name: String,
    pub repo_id: u64,
    pub owner_id: u64,
    pub installation_id: u64,
    pub slug: String,
    pub client_id: String,
    pub probe_token: String,
    pub bot_token: String,
    pub scoped_token: String,
    pub user_access: String,
    pub user_refresh: String,
    pub can_push: bool,
}

impl RepoFixture {
    pub fn new(owner: &str, name: &str) -> RepoFixture {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let tag = format!(
            "{:x}{:x}{nanos:x}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        );
        RepoFixture {
            owner: owner.into(),
            name: name.into(),
            repo_id: 7001,
            owner_id: 9001,
            installation_id: 31337,
            slug: "moor-bot".into(),
            client_id: "Iv1.parity".into(),
            probe_token: format!("ghs_PROBE_{tag}"),
            bot_token: format!("ghs_BOT_{tag}"),
            scoped_token: format!("ghu_SCOPED_{tag}"),
            user_access: format!("ghu_ACCESS_{tag}"),
            user_refresh: format!("ghr_REFRESH_{tag}"),
            can_push: true,
        }
    }

    pub fn repo_path(&self) -> String {
        format!("/repos/{}/{}", self.owner, self.name)
    }

    pub fn repo_json(&self) -> Value {
        json!({
            "id": self.repo_id,
            "name": self.name,
            "owner": { "login": self.owner, "id": self.owner_id },
            "permissions": { "push": self.can_push },
        })
    }

    pub fn tokens_json(&self) -> Value {
        self.tokens(&self.user_refresh, 28800)
    }

    pub fn tokens(&self, refresh: &str, access_expires_in: u64) -> Value {
        json!({
            "access_token": self.user_access,
            "expires_in": access_expires_in,
            "refresh_token": refresh,
            "refresh_token_expires_in": 15_552_000,
        })
    }
}

/// In-process GitHub stand-in on 127.0.0.1:0. Routes are scripted per test,
/// every request is recorded, and the listener and its threads stop on Drop.
pub struct FakeGithub {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

impl FakeGithub {
    pub fn start() -> FakeGithub {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake github");
        let addr = listener.local_addr().expect("local addr");
        let shared = Arc::new(Shared {
            routes: Mutex::new(Vec::new()),
            log: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        let accept = {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || accept_loop(listener, shared))
        };
        FakeGithub {
            addr,
            shared,
            accept: Some(accept),
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn env(&self) -> Vec<(String, String)> {
        ["MOOR_GITHUB_API", "MOOR_GITHUB_WEB"]
            .iter()
            .map(|k| (k.to_string(), self.url()))
            .collect()
    }

    /// Points every later moor call in the world at this server.
    pub fn attach(&self, w: &World) {
        for (k, v) in self.env() {
            w.set_env(&k, &v);
        }
    }

    /// Answers `method path` with `replies` in order; the last reply repeats.
    /// A path ending in `*` is a prefix route, tried only when no exact route matches.
    pub fn route(&self, method: &str, path: &str, replies: Vec<Reply>) {
        self.push_route(method, path, None, replies);
    }

    /// Like `route`, matching only requests whose body contains `body_has`.
    pub fn route_when(&self, method: &str, path: &str, body_has: &str, replies: Vec<Reply>) {
        self.push_route(method, path, Some(body_has), replies);
    }

    /// Replaces any route with the same method, path and body filter.
    pub fn reroute(&self, method: &str, path: &str, replies: Vec<Reply>) {
        self.replace(method, path, None, replies);
    }

    pub fn reroute_when(&self, method: &str, path: &str, body_has: &str, replies: Vec<Reply>) {
        self.replace(method, path, Some(body_has), replies);
    }

    fn replace(&self, method: &str, path: &str, body_has: Option<&str>, replies: Vec<Reply>) {
        self.shared.routes.lock().unwrap().retain(|r| {
            !(r.method == method && r.path == path && r.body_has.as_deref() == body_has)
        });
        self.push_route(method, path, body_has, replies);
    }

    fn push_route(&self, method: &str, path: &str, body_has: Option<&str>, replies: Vec<Reply>) {
        assert!(!replies.is_empty(), "route {method} {path} has no reply");
        self.shared.routes.lock().unwrap().push(Route {
            method: method.to_string(),
            path: path.to_string(),
            body_has: body_has.map(str::to_string),
            replies,
            hits: 0,
        });
    }

    /// Routes for the App, one installed repo, and the user device and refresh
    /// flows; any other repo answers 403.
    pub fn script_repo(&self, fx: &RepoFixture) {
        let repo = fx.repo_path();
        self.route("GET", "/app", vec![Reply::ok(&json!({ "slug": fx.slug }))]);
        self.route(
            "GET",
            &format!("{repo}/installation"),
            vec![Reply::ok(&json!({ "id": fx.installation_id }))],
        );
        self.script_installation_tokens(fx);
        self.route("GET", &repo, vec![Reply::ok(&fx.repo_json())]);
        self.route(
            "POST",
            &format!("/applications/{}/token/scoped", fx.client_id),
            vec![Reply::ok(&json!({ "token": fx.scoped_token }))],
        );
        self.script_device_flow(fx);
        self.route_when(
            "POST",
            "/login/oauth/access_token",
            "refresh_token",
            vec![Reply::ok(&fx.tokens_json())],
        );
        self.deny_other_repos();
    }

    pub fn script_installation_tokens(&self, fx: &RepoFixture) {
        let path = format!("/app/installations/{}/access_tokens", fx.installation_id);
        let expiry = "2099-01-01T00:00:00Z";
        self.route_when(
            "POST",
            &path,
            "metadata",
            vec![Reply::json(
                201,
                &json!({ "token": fx.probe_token, "expires_at": expiry }),
            )],
        );
        self.route_when(
            "POST",
            &path,
            "repository_ids",
            vec![Reply::json(
                201,
                &json!({ "token": fx.bot_token, "expires_at": expiry }),
            )],
        );
    }

    pub fn script_device_flow(&self, fx: &RepoFixture) {
        let code = json!({
            "device_code": "dc_parity",
            "user_code": "PARI-TY00",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 0,
        });
        self.route("POST", "/login/device/code", vec![Reply::ok(&code)]);
        self.route_when(
            "POST",
            "/login/oauth/access_token",
            "device_code",
            vec![Reply::ok(&fx.tokens_json())],
        );
    }

    pub fn deny_other_repos(&self) {
        let deny = || {
            vec![Reply::message(
                403,
                "Resource not accessible by integration",
            )]
        };
        self.route("GET", "/repos/*", deny());
        self.route("POST", "/repos/*", deny());
    }

    pub fn requests(&self) -> Vec<Request> {
        self.shared.log.lock().unwrap().clone()
    }

    pub fn requests_to(&self, method: &str, path: &str) -> Vec<Request> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == method && r.path == path)
            .collect()
    }

    pub fn clear_requests(&self) {
        self.shared.log.lock().unwrap().clear();
    }
}

impl Drop for FakeGithub {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    for stream in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        let shared = Arc::clone(&shared);
        workers.push(std::thread::spawn(move || serve(stream, &shared)));
    }
    for w in workers {
        let _ = w.join();
    }
}

fn serve(mut stream: TcpStream, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Some(req) = read_request(&mut stream) else {
        return;
    };
    shared.log.lock().unwrap().push(req.clone());
    let reply = pick(shared, &req);
    let reason = if reply.status < 300 { "OK" } else { "X" };
    let location = reply
        .location
        .as_ref()
        .map(|l| format!("Location: {l}\r\n"))
        .unwrap_or_default();
    let head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n",
        reply.status,
        reply.body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(reply.body.as_bytes());
}

fn pick(shared: &Shared, req: &Request) -> Reply {
    let mut routes = shared.routes.lock().unwrap();
    for wild in [false, true] {
        if let Some(r) = routes.iter_mut().find(|r| r.matches(req, wild)) {
            let reply = r.replies[r.hits.min(r.replies.len() - 1)].clone();
            r.hits += 1;
            return reply;
        }
    }
    Reply::message(404, "Not Found")
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.split('?').next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':')?;
        headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
    }
    let len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

#[cfg(test)]
mod tests;
