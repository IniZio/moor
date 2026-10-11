use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};

use crate::world::World;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    pub fn bearer(&self) -> Option<&str> {
        self.header("authorization")?.strip_prefix("Bearer ")
    }

    /// Decoded (header, claims) of the Bearer JWT; None when the token is not a JWT.
    pub fn jwt(&self) -> Option<(Value, Value)> {
        let mut parts = self.bearer()?.split('.');
        let (h, c, _sig) = (parts.next()?, parts.next()?, parts.next()?);
        let decode = |s: &str| serde_json::from_slice(&base64url_decode(s)?).ok();
        Some((decode(h)?, decode(c)?))
    }

    pub fn basic(&self) -> Option<String> {
        let raw = self.header("authorization")?.strip_prefix("Basic ")?;
        String::from_utf8(base64url_decode(raw)?).ok()
    }
}

/// Decodes standard or URL-safe base64, padded or not.
pub fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub location: Option<String>,
}

impl Reply {
    pub fn json(status: u16, body: &Value) -> Reply {
        Reply {
            status,
            body: body.to_string(),
            location: None,
        }
    }

    pub fn ok(body: &Value) -> Reply {
        Reply::json(200, body)
    }

    pub fn redirect(status: u16, location: &str) -> Reply {
        Reply {
            status,
            body: String::new(),
            location: Some(location.to_string()),
        }
    }

    pub fn message(status: u16, message: &str) -> Reply {
        Reply::json(status, &json!({ "message": message }))
    }

    pub fn oauth_error(error: &str) -> Reply {
        Reply::ok(&json!({ "error": error }))
    }
}

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

/// RSA test key for signing App JWTs; it protects nothing.
pub const TEST_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDCcuYfTC8Mvd4H\n\
kZjCD/oDAMCwXpdK4Gyjn4D0fVT5nrqnaHgkU4Oku2q5iUddCwPolj2HeCiYurB2\n\
NnAqVCGCQY1ZxEq6SHjQqnVFutTdk1AUyQXYbBBNrQ0f+Vd7de5aTgb5nJNilQsF\n\
AGXHt2Vi+ZYKHI9n3f/rxV+D7q48pgm+TXFlsdOvCwvQT36FRaUAQPhy7RuweLkb\n\
8g9t1FgRDV7WKHznyWAHi/1sFpdESogTCl/NsAJlo6+XfWp3mD7Q5WbMIXIhZj3Z\n\
X8Df78nxE3BrYjo6WOTkYbLf+qj8shi0Z5HchIew61cci3VeQj04jNbIZwYpQ3cW\n\
RYL4FanTAgMBAAECggEAMsLU8UGtnKlqL1aiZrLOBP+qzb/7xee9ZJ+Mhga76qg1\n\
uOMsTethV903OLUuJItuG0cxS86h5nECBPIkKOCqmLrtIAx+p4+ovmxpTek+jZNZ\n\
2fs6Y+ud+/Ebloe8fzXjob5x0rK9xQP8ivM2HsNxG7vlH8Gh7C8HkrCFRUdHQ/xt\n\
wpl6AwAT051dE0zT2BQ9R/fxxluIqBxSpDkIo3dxRUrt6mRT3/20LKf/5vUK+u3D\n\
/huerjKDiDVIKIBnlyF4zMTy2Kyu7zI7ed0v2ajzeHHr/LvdJD5YjO9pUzy1G0/E\n\
GQOHvyrtwlD8Xh6hIG6TGYWYwZnSNiIKa00SbLlLcQKBgQDonfUKxkez4chwOhOQ\n\
n/2hrU/luPqGLU+uSNUdjMdrfx3PexGezbumjqtjf28XFtOIJs8nPOBGCfSxSqDt\n\
zQWT1Y5gbcQp+leuiget9oHCkRwW/KJvRONIZeMuVe09N36Lt3Goqi34T9ggCw1R\n\
TznHVEHqDeGmmhWwwGi4aUPIAwKBgQDV/r3bprU0jZS3VXlrPL+G8hxUFtAV/DqX\n\
KjwA16jn3E7X4r5ANDsTazaWTc/dfUK0PXzrrAEgUyRtcYznNkyrpssfoeDh9HLL\n\
6PKU+a7VP9Gx9fXCiGoVzC1HKFGp3il/KlHqn2c1NsrjG4h95cicjjiALoP4M2mX\n\
AFUOOZ918QKBgFYnksdv0sTD/inayL5LE3RbLXlm/prNk3J4lT5Bkua0iQT8jvUj\n\
O7ofo0jRvQJvON93iEf/Gw21yEQV4O7SfZtYgEgkv3/QscL6aoe7Aipe9mlpHnxZ\n\
plh9LBxQRPLkZ+E1i0ot5aLPZ7vOfBs+Xab/BP68L3Q2I4x4ZxDOJAWjAoGAYKUh\n\
3beWktt2VK43neTbovhpLtj2tVcqpuaeHstHLx/iQ8HVpm1wWL7M8mjF/gP6Qiia\n\
6FzQTWb/RqZHuoxdaBB2IB0d0S5zQgA8vmddPFYMQFES4v502s9Ri5d7+jmpiBL2\n\
1/2RwQiz7ImcxS8nwPzCK7HWTBv8DgRKZZhEmiECgYEAjerbV+C43Yx2ucXhUdY+\n\
q01qMNfEq6+ySaN1fMllK68FwTO6YD8M/WP2CbNHARw/nGwnMVmjE35KnXW7+17r\n\
abb7PC1AfiOSFSC/sUHQDzYofWtOW9smkd+rekkNDwho0XR49jl3pfcx3qnvIY5u\n\
+9uvTEScVx4SgNaWmuOluR0=\n\
-----END PRIVATE KEY-----\n\
";

#[cfg(test)]
mod tests {
    use super::*;

    fn get(addr: &str, path: &str) -> (u16, String) {
        let mut s = TcpStream::connect(addr).unwrap();
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer a.b.c\r\n\r\n"
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let status = out[9..12].parse().unwrap();
        (
            status,
            out.split("\r\n\r\n").nth(1).unwrap_or("").to_string(),
        )
    }

    #[test]
    fn exact_wins_over_prefix_and_last_reply_repeats() {
        let g = FakeGithub::start();
        g.route("GET", "/repos/*", vec![Reply::message(403, "no")]);
        g.route(
            "GET",
            "/repos/a/b",
            vec![Reply::ok(&json!({"n": 1})), Reply::ok(&json!({"n": 2}))],
        );
        let addr = g.url().replace("http://", "");
        assert_eq!(get(&addr, "/repos/a/b").1, r#"{"n":1}"#);
        assert_eq!(get(&addr, "/repos/a/b").1, r#"{"n":2}"#);
        assert_eq!(get(&addr, "/repos/a/b").1, r#"{"n":2}"#);
        assert_eq!(get(&addr, "/repos/other/c").0, 403);
        assert_eq!(get(&addr, "/nope").0, 404);
        assert_eq!(g.requests().len(), 5);
        assert_eq!(g.requests()[0].bearer(), Some("a.b.c"));
    }

    #[test]
    fn base64_forms_decode() {
        assert_eq!(base64url_decode("aGk").unwrap(), b"hi");
        assert_eq!(base64url_decode("aGk=").unwrap(), b"hi");
        assert_eq!(base64url_decode("_-8").unwrap(), [0xff, 0xef]);
        assert!(base64url_decode("a b").is_none());
    }
}
