//! A fake GitHub REST API on loopback and an RSA key made once per test run, for the App
//! client and the reads it makes.
#![allow(clippy::unwrap_used, reason = "test double")]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rsa::{KeyPair, KeySize, PublicKeyComponents};
use aws_lc_rs::signature::KeyPair as _;
use serde_json::{json, Value};

use super::AppClient;

/// A test RSA key: its PKCS#1 PEM, its PKCS#8 PEM and its public modulus and exponent.
pub(crate) struct TestKey {
    /// PKCS#1 PEM, as GitHub generates.
    pub(crate) pkcs1: String,
    /// PKCS#8 PEM.
    pub(crate) pkcs8: String,
    /// Public modulus.
    pub(crate) n: Vec<u8>,
    /// Public exponent.
    pub(crate) e: Vec<u8>,
}

/// One 2048-bit key per test run; RSA key generation is slow.
pub(crate) fn key() -> &'static TestKey {
    static KEY: OnceLock<TestKey> = OnceLock::new();
    KEY.get_or_init(|| {
        let pair = KeyPair::generate(KeySize::Rsa2048).unwrap();
        let pkcs8 = pair.as_der().unwrap().as_ref().to_vec();
        // PrivateKeyInfo: SEQUENCE { version, rsaEncryption AlgorithmIdentifier, OCTET STRING {
        // RSAPrivateKey } }. For a 2048-bit key the OCTET STRING header ends at byte 26.
        assert_eq!(
            pkcs8.get(22..24),
            Some(&[0x04, 0x82][..]),
            "unexpected PKCS#8 layout"
        );
        let pkcs1 = pkcs8.get(26..).unwrap().to_vec();
        let public = PublicKeyComponents::<Vec<u8>>::from(pair.public_key());
        TestKey {
            pkcs1: pem::encode(&pem::Pem::new("RSA PRIVATE KEY", pkcs1)),
            pkcs8: pem::encode(&pem::Pem::new("PRIVATE KEY", pkcs8)),
            n: public.n,
            e: public.e,
        }
    })
}

/// One request the fake saw.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    /// `GET` or `POST`.
    pub(crate) method: String,
    /// Path and query.
    pub(crate) path: String,
    /// The `Authorization` header.
    pub(crate) auth: String,
    /// The body.
    pub(crate) body: String,
}

/// A fake GitHub REST API: `route` answers each request by method and path.
pub(crate) struct FakeGitHub {
    /// Where it listens, `http://127.0.0.1:PORT`.
    pub(crate) url: String,
    /// Every request it answered.
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl FakeGitHub {
    /// A fake on loopback answering with `route`.
    pub(crate) fn start(route: impl Fn(&str, &str) -> (u16, Value) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut reader = BufReader::new(stream);
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut parts = first.split_whitespace();
                let method = parts.next().unwrap_or_default().to_owned();
                let path = parts.next().unwrap_or_default().to_owned();
                let (mut length, mut auth) = (0, String::new());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let lower = line.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("authorization") {
                            value.trim().clone_into(&mut auth);
                        }
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body = String::from_utf8(body).unwrap();
                let (status, reply) = route(&method, &path);
                log.lock().unwrap().push(Seen {
                    method,
                    path,
                    auth,
                    body,
                });
                let reply = reply.to_string();
                let mut stream = reader.into_inner();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Answer\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Self { url, seen }
    }

    /// Every request it answered so far.
    pub(crate) fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// An App client, id 42 with the test key, calling this fake.
    pub(crate) fn client(&self) -> AppClient {
        AppClient::from_pem(42, key().pkcs1.as_bytes(), &self.url).unwrap()
    }
}

/// Pull requests and collaborator permissions as a fake GitHub holds them, read through one
/// installation on every repo, which the App reads back.
pub(crate) struct PullsApi {
    /// The fake.
    pub(crate) api: FakeGitHub,
    /// Each pull request by number, on whatever repo it is asked for.
    pulls: Arc<Mutex<HashMap<u64, Value>>>,
    /// Each login's `permission`; `none` for one not here.
    permissions: Arc<Mutex<HashMap<String, String>>>,
    /// Whether every request answers `503`.
    down: Arc<AtomicBool>,
}

impl PullsApi {
    /// A fake with no pull requests and no collaborators.
    pub(crate) fn start() -> Self {
        let pulls: Arc<Mutex<HashMap<u64, Value>>> = Arc::default();
        let permissions: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        let down = Arc::new(AtomicBool::new(false));
        let (p, w, d) = (
            Arc::clone(&pulls),
            Arc::clone(&permissions),
            Arc::clone(&down),
        );
        let api = FakeGitHub::start(move |method, path| {
            if d.load(Ordering::SeqCst) {
                return (503, json!({"message": "Service Unavailable"}));
            }
            let parts: Vec<&str> = path.split('/').collect();
            match (method, parts.as_slice()) {
                ("GET", ["", "repos", _, _, "installation"]) => (200, json!({"id": 1})),
                ("POST", ["", "app", "installations", "1", "access_tokens"]) => {
                    (201, json!({"token": "ghs_test"}))
                }
                ("GET", ["", "repos", _, _, "pulls", n]) => n
                    .parse()
                    .ok()
                    .and_then(|n: u64| p.lock().unwrap().get(&n).cloned())
                    .map_or_else(|| (404, json!({"message": "Not Found"})), |v| (200, v)),
                ("GET", ["", "repos", _, _, "collaborators", login, "permission"]) => {
                    let found = w.lock().unwrap().get(*login).cloned();
                    (
                        200,
                        json!({"permission": found.unwrap_or_else(|| "none".to_owned())}),
                    )
                }
                _ => (404, json!({"message": "Not Found"})),
            }
        });
        Self {
            api,
            pulls,
            permissions,
            down,
        }
    }

    /// Sets pull request `number` to `pull`, as [`pull_json`] builds it.
    pub(crate) fn set_pull(&self, number: u64, pull: Value) {
        self.pulls.lock().unwrap().insert(number, pull);
    }

    /// Gives `login` `permission` on every repo.
    pub(crate) fn set_permission(&self, login: &str, permission: &str) {
        self.permissions
            .lock()
            .unwrap()
            .insert(login.to_owned(), permission.to_owned());
    }

    /// Makes every request answer `503` while `down`.
    pub(crate) fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }
}

/// A pull request as `GET /repos/{o}/{r}/pulls/{n}` answers it: `open` or closed, a draft or
/// not, at `head` in `head_repo`, by `login` of type `kind`.
pub(crate) fn pull_json(
    (open, draft): (bool, bool),
    (head, head_repo): (&str, &str),
    (login, kind): (&str, &str),
) -> Value {
    json!({
        "state": if open { "open" } else { "closed" },
        "draft": draft,
        "head": {"sha": head, "repo": {"full_name": head_repo}},
        "user": {"login": login, "type": kind},
    })
}
