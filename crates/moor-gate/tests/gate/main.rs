//! The gate against an in-process OIDC issuer and a fake controller front socket that echoes
//! every line it receives back to the client.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::disallowed_methods,
    clippy::needless_pass_by_value,
    reason = "tests"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use moor_gate::{Gate, Verifier};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const CLIENT_ID: &str = "moor-test-client";

struct World {
    issuer: String,
    gate: String,
    key: Vec<u8>,
    /// The issuer's userinfo answer: its status and body.
    userinfo: Arc<Mutex<(u16, Value)>>,
    /// Userinfo requests that carried a bearer token.
    userinfo_calls: Arc<AtomicUsize>,
    /// Lines the fake controller received, in order; `"<eof>"` when the client half-closed.
    seen: mpsc::Receiver<String>,
    _dir: tempfile::TempDir,
}

fn keypair() -> Vec<u8> {
    EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
        .unwrap()
        .as_ref()
        .to_vec()
}

fn jwks(pkcs8: &[u8]) -> Value {
    let pair = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        pkcs8,
        &SystemRandom::new(),
    )
    .unwrap();
    let point = pair.public_key().as_ref();
    json!({"keys": [{"kty": "EC", "crv": "P-256", "kid": "k1", "alg": "ES256", "use": "sig",
        "x": URL_SAFE_NO_PAD.encode(&point[1..33]), "y": URL_SAFE_NO_PAD.encode(&point[33..65])}]})
}

fn serve_issuer(
    pkcs8: &[u8],
    userinfo: Arc<Mutex<(u16, Value)>>,
    calls: Arc<AtomicUsize>,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (keys, discovery) = (
        jwks(pkcs8).to_string(),
        json!({"jwks_uri": format!("{base}/jwks"), "userinfo_endpoint": format!("{base}/userinfo")})
            .to_string(),
    );
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let mut line = String::new();
            let mut bearer = false;
            while reader.read_line(&mut line).unwrap() > 2 {
                bearer |= line
                    .to_ascii_lowercase()
                    .starts_with("authorization: bearer ");
                line.clear();
            }
            let (status, body) = if first.contains("/jwks") {
                (200, keys.clone())
            } else if first.contains("/userinfo") {
                if bearer {
                    calls.fetch_add(1, Ordering::SeqCst);
                }
                let (status, body) = &*userinfo.lock().unwrap();
                (*status, body.to_string())
            } else {
                (200, discovery.clone())
            };
            let mut stream = reader.into_inner();
            let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    base
}

fn serve_controller(path: &std::path::Path) -> mpsc::Receiver<String> {
    let listener = UnixListener::bind(path).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reply = stream.try_clone().unwrap();
            let mut lines = BufReader::new(stream).lines();
            while let Some(Ok(line)) = lines.next() {
                writeln!(reply, "{line}").unwrap();
                tx.send(line).unwrap();
            }
            let _ = tx.send("<eof>".to_owned());
        }
    });
    rx
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let seen = serve_controller(&socket);
    world_on(socket, seen, dir)
}

fn world_on(
    socket: std::path::PathBuf,
    seen: mpsc::Receiver<String>,
    dir: tempfile::TempDir,
) -> World {
    world_with(socket, seen, dir, false)
}

fn world_with(
    socket: std::path::PathBuf,
    seen: mpsc::Receiver<String>,
    dir: tempfile::TempDir,
    mcp: bool,
) -> World {
    let key = keypair();
    let userinfo = Arc::new(Mutex::new((
        200,
        json!({"sub": "alice-sub", "email": "Alice@Example.com", "email_verified": true}),
    )));
    let userinfo_calls = Arc::new(AtomicUsize::new(0));
    let issuer = serve_issuer(&key, userinfo.clone(), userinfo_calls.clone());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let gate = listener.local_addr().unwrap().to_string();
    let mcp =
        mcp.then(|| moor_gate::Mcp::new(&format!("http://{gate}/mcp"), socket.clone()).unwrap());
    let config = Arc::new(Gate {
        front_socket: socket,
        mcp,
        verifier: Verifier::new(issuer.clone(), CLIENT_ID.to_owned(), None),
    });
    std::thread::spawn(move || moor_gate::serve(&listener, &config));
    World {
        issuer,
        gate,
        key,
        userinfo,
        userinfo_calls,
        seen,
        _dir: dir,
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn claims(w: &World) -> Value {
    json!({"iss": w.issuer, "aud": CLIENT_ID, "sub": "alice-sub", "exp": now() + 600,
        "email": "Alice@Example.com", "email_verified": true})
}

fn sign(key: &[u8], claims: &Value) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("k1".to_owned());
    encode(&header, claims, &EncodingKey::from_ec_der(key)).unwrap()
}

/// An RFC 9068 access token as Authgear issues it: the issuer as audience, the client in
/// `client_id`, and no email claims.
fn authgear_token(w: &World) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("k1".to_owned());
    header.typ = Some("at+jwt".to_owned());
    let claims = json!({"iss": w.issuer, "aud": [w.issuer], "client_id": CLIENT_ID,
        "sub": "alice-sub", "exp": now() + 600, "iat": now(), "auth_time": now(), "jti": "j1",
        "scope": "openid email offline_access",
        "https://authgear.com/claims/user/is_anonymous": false});
    encode(&header, &claims, &EncodingKey::from_ec_der(&w.key)).unwrap()
}

/// The 401 body the gate answers `token` with at `/front`.
fn refusal(w: &World, token: &str) -> String {
    let mut stream = TcpStream::connect(&w.gate).unwrap();
    write!(
        stream,
        "GET /front HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 401"), "{out}");
    out.split("\r\n\r\n").nth(1).unwrap().to_owned()
}

type Client = WebSocket<MaybeTlsStream<TcpStream>>;

fn connect(w: &World, token: Option<&str>) -> Result<Client, u16> {
    let mut req = format!("ws://{}/front", w.gate)
        .into_client_request()
        .unwrap();
    if let Some(token) = token {
        req.headers_mut()
            .insert("Authorization", format!("Bearer {token}").parse().unwrap());
    }
    match tungstenite::connect(req) {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(res)) => Err(res.status().as_u16()),
        Err(e) => panic!("connect failed: {e}"),
    }
}

fn send(ws: &mut Client, line: &Value) {
    ws.send(Message::binary(format!("{line}\n").into_bytes()))
        .unwrap();
}

fn recv(ws: &mut Client) -> Value {
    let mut buf = Vec::new();
    while !buf.ends_with(b"\n") {
        match ws.read().unwrap() {
            Message::Binary(b) => buf.extend_from_slice(&b),
            other => panic!("unexpected {other:?}"),
        }
    }
    serde_json::from_slice(&buf).unwrap()
}

fn request(id: &str, request: Value) -> Value {
    json!({"frame": "request", "id": id, "request": request})
}

#[test]
fn a_verified_token_acts_as_its_identity_on_every_request() {
    let w = world();
    let mut ws = connect(&w, Some(&sign(&w.key, &claims(&w)))).unwrap();
    send(&mut ws, &json!({"frame": "hello", "protocol": 15}));
    assert_eq!(recv(&mut ws), json!({"frame": "hello", "protocol": 15}));
    send(&mut ws, &request("1", json!({"cmd": "whoami"})));
    let as_alice =
        json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {"cmd": "whoami"}});
    assert_eq!(recv(&mut ws), request("1", as_alice));
    send(&mut ws, &request("2", json!({"cmd": "enroll"})));
    let enroll = json!({"cmd": "as", "identity": "authgear:alice-sub",
        "request": {"cmd": "enroll", "email": "alice@example.com"}});
    assert_eq!(recv(&mut ws), request("2", enroll));
    send(&mut ws, &json!({"frame": "watch", "since": 0}));
    assert_eq!(
        recv(&mut ws),
        json!({"frame": "watch", "since": 0, "identity": "authgear:alice-sub"})
    );
}

#[test]
fn a_frame_asserting_another_identity_never_reaches_the_controller() {
    let w = world();
    let mut ws = connect(&w, Some(&sign(&w.key, &claims(&w)))).unwrap();
    let spoofs = [
        json!({"cmd": "whoami", "identity": "authgear:mallory"}),
        json!({"cmd": "as", "identity": "authgear:mallory", "request": {"cmd": "sessions"}}),
        json!({"cmd": "as", "identity": "slack:T1/U1", "request": {"cmd": "sessions"}}),
        json!({"cmd": "enroll", "email": "mallory@example.com"}),
    ];
    for (n, spoof) in spoofs.into_iter().enumerate() {
        send(&mut ws, &request(&n.to_string(), spoof));
        let got = recv(&mut ws);
        assert_eq!(got["frame"], "response", "{got}");
        assert_eq!(got["envelope"]["error"]["kind"], "refused", "{got}");
    }
    send(
        &mut ws,
        &json!({"frame": "watch", "since": 0, "identity": "authgear:mallory"}),
    );
    assert_eq!(recv(&mut ws)["frame"], "error");
    send(&mut ws, &request("ok", json!({"cmd": "whoami"})));
    recv(&mut ws);
    assert_eq!(
        w.seen.recv().unwrap(),
        request(
            "ok",
            json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {"cmd": "whoami"}})
        )
        .to_string()
    );
}

#[test]
fn an_empty_message_half_closes_the_controller_socket() {
    let w = world();
    let mut ws = connect(&w, Some(&sign(&w.key, &claims(&w)))).unwrap();
    send(&mut ws, &json!({"frame": "hello", "protocol": 15}));
    recv(&mut ws);
    ws.send(Message::binary(Vec::new())).unwrap();
    assert_eq!(
        w.seen.recv().unwrap(),
        json!({"frame": "hello", "protocol": 15}).to_string()
    );
    assert_eq!(w.seen.recv().unwrap(), "<eof>");
    assert_eq!(
        ws.read().unwrap(),
        Message::binary(Vec::new()),
        "the controller's half-close"
    );
    assert!(
        matches!(ws.read().unwrap(), Message::Close(_)),
        "then the gate closes"
    );
}

#[test]
fn bad_tokens_get_401_before_the_upgrade() {
    let w = world();
    *w.userinfo.lock().unwrap() = (200, json!({"sub": "alice-sub"}));
    let other_key = keypair();
    let mut wrong_aud = claims(&w);
    wrong_aud["aud"] = json!("someone-else");
    let mut expired = claims(&w);
    expired["exp"] = json!(now() - 3600);
    let mut unverified = claims(&w);
    unverified["email_verified"] = json!(false);
    let mut no_email = claims(&w);
    no_email.as_object_mut().unwrap().remove("email");
    let mut wrong_iss = claims(&w);
    wrong_iss["iss"] = json!("http://issuer.example.com");
    for (what, token) in [
        ("wrong aud", sign(&w.key, &wrong_aud)),
        ("expired", sign(&w.key, &expired)),
        ("bad signature", sign(&other_key, &claims(&w))),
        ("unverified email", sign(&w.key, &unverified)),
        ("no email", sign(&w.key, &no_email)),
        ("wrong iss", sign(&w.key, &wrong_iss)),
        ("not a jwt", "garbage".to_owned()),
    ] {
        assert_eq!(connect(&w, Some(&token)).err(), Some(401), "{what}");
    }
    assert_eq!(connect(&w, None).err(), Some(401), "no token");
}

#[test]
fn an_authgear_access_token_acts_with_the_userinfo_email() {
    let w = world();
    let mut ws = connect(&w, Some(&authgear_token(&w))).unwrap();
    send(&mut ws, &request("1", json!({"cmd": "enroll"})));
    let enroll = json!({"cmd": "as", "identity": "authgear:alice-sub",
        "request": {"cmd": "enroll", "email": "alice@example.com"}});
    assert_eq!(recv(&mut ws), request("1", enroll));
    assert_eq!(w.userinfo_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_second_sign_in_reuses_the_userinfo_email() {
    let w = world();
    connect(&w, Some(&authgear_token(&w))).unwrap();
    connect(&w, Some(&authgear_token(&w))).unwrap();
    assert_eq!(w.userinfo_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_dex_token_with_a_verified_email_never_asks_userinfo() {
    let w = world();
    connect(&w, Some(&sign(&w.key, &claims(&w)))).unwrap();
    assert_eq!(w.userinfo_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn an_unverified_userinfo_email_is_refused() {
    let w = world();
    *w.userinfo.lock().unwrap() = (
        200,
        json!({"sub": "alice-sub", "email": "alice@example.com", "email_verified": false}),
    );
    let why = refusal(&w, &authgear_token(&w));
    assert!(why.contains("alice@example.com is not verified"), "{why}");
}

#[test]
fn userinfo_without_an_email_is_refused() {
    let w = world();
    *w.userinfo.lock().unwrap() = (200, json!({"sub": "alice-sub"}));
    let why = refusal(&w, &authgear_token(&w));
    assert!(why.contains("gives no email"), "{why}");
}

#[test]
fn userinfo_for_another_subject_is_refused() {
    let w = world();
    *w.userinfo.lock().unwrap() = (
        200,
        json!({"sub": "mallory-sub", "email": "mallory@example.com", "email_verified": true}),
    );
    let why = refusal(&w, &authgear_token(&w));
    assert!(why.contains("another subject"), "{why}");
}

#[test]
fn a_failing_userinfo_endpoint_is_refused_with_why() {
    let w = world();
    *w.userinfo.lock().unwrap() = (503, json!({}));
    let why = refusal(&w, &authgear_token(&w));
    assert!(
        why.contains("the token carries no email and asking userinfo failed"),
        "{why}"
    );
    assert!(why.contains("503"), "{why}");
}

#[test]
fn a_client_id_claim_for_another_client_is_refused() {
    let w = world();
    let mut c = claims(&w);
    c["aud"] = json!([w.issuer]);
    c["client_id"] = json!("someone-else");
    let why = refusal(&w, &sign(&w.key, &c));
    assert!(why.contains("another audience"), "{why}");
}

fn http_get(w: &World, path: &str) -> String {
    let mut stream = TcpStream::connect(&w.gate).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn the_metadata_names_the_issuer_and_client() {
    let w = world();
    let res = http_get(&w, "/.well-known/moor");
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let body: Value = serde_json::from_str(res.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"issuer": w.issuer, "client_id": CLIENT_ID,
        "scopes": ["openid", "email", "offline_access"]})
    );
}

#[test]
fn any_other_path_is_404() {
    let w = world();
    for path in ["/", "/mcp", "/front/x", "/.well-known/openid-configuration"] {
        assert!(http_get(&w, path).starts_with("HTTP/1.1 404"), "{path}");
    }
}

/// Runs only with `MOOR_BIN` naming a built `moor`: a real controller with an `authgear` front
/// enrolls the gate's person with the token's email.
#[test]
#[ignore = "needs MOOR_BIN, a built moor binary"]
fn a_real_controller_enrolls_the_token_email() {
    let moor = std::env::var("MOOR_BIN").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("c.yaml"),
        format!(
            "socket: {}\nstate_dir: {}\nfronts:\n  authgear: {{}}\n",
            root.join("c.sock").display(),
            root.join("state").display()
        ),
    )
    .unwrap();
    let mut controller = std::process::Command::new(moor)
        .args(["controller", "serve", "--config"])
        .arg(root.join("c.yaml"))
        .env("HOME", root)
        .spawn()
        .unwrap();
    let state = root.join("state");
    let socket = state.join("fronts/authgear/front");
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let (_tx, seen) = mpsc::channel();
    let w = world_on(socket, seen, tempfile::tempdir().unwrap());
    let mut ws = connect(&w, Some(&sign(&w.key, &claims(&w)))).unwrap();
    send(
        &mut ws,
        &json!({"frame": "hello", "protocol": moor_core::controller::PROTOCOL}),
    );
    recv(&mut ws);
    send(&mut ws, &request("e", json!({"cmd": "enroll"})));
    let enrolled = recv(&mut ws);
    send(&mut ws, &request("w", json!({"cmd": "whoami"})));
    let whoami = recv(&mut ws);
    controller.kill().unwrap();
    controller.wait().unwrap();
    assert_eq!(enrolled["envelope"]["class"], "ok", "{enrolled}");
    assert_eq!(
        whoami["envelope"]["value"]["identity"], "authgear:alice-sub",
        "{whoami}"
    );
    let db = rusqlite::Connection::open(state.join("moor.db")).unwrap();
    let email: String = db
        .query_row("SELECT email FROM users", [], |row| row.get(0))
        .unwrap();
    assert_eq!(email, "alice@example.com");
}

mod enroll;
mod mcp;
