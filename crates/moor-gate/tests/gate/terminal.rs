//! The portal terminal at `/account/api/box/terminal`: a WebSocket the session cookie signs
//! in, refused from another origin or without the page's token, that runs `attach` as the
//! session's person against a fake controller which echoes keys and reports sizes.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

use moor_core::controller::{parse_frame, write_bytes, ClientFrame};
use moor_core::response::{ErrorKind, ErrorView, Response};
use moor_gate::{Gate, Login, Verifier};
use moor_testkit::oidc::Issuer;
use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;

use super::login::{self, get, sign_in};
use super::CLIENT_ID;

const BOX: &str = "web/fix";

struct World {
    addr: String,
    issuer: Issuer,
    /// Every `attach` the controller got, inside its `as`.
    seen: Arc<Mutex<Vec<Value>>>,
    _dir: tempfile::TempDir,
}

/// Answers one connection: a refusal for anyone's Box but `alice-sub`'s, else a terminal that
/// echoes each key and prints each new size.
fn serve(stream: &UnixStream, seen: &Mutex<Vec<Value>>) {
    let mut out = stream.try_clone().unwrap();
    let mut read = BufReader::new(stream);
    let mut line = String::new();
    read.read_line(&mut line).unwrap();
    writeln!(out, "{}", line.trim_end()).unwrap();
    line.clear();
    read.read_line(&mut line).unwrap();
    let frame: Value = serde_json::from_str(&line).unwrap();
    let (id, request) = (
        frame["id"].as_str().unwrap().to_owned(),
        frame["request"].clone(),
    );
    seen.lock().unwrap().push(request.clone());
    let reply = |out: &mut UnixStream, envelope: Response| {
        let frame = json!({"frame": "response", "id": id, "envelope": envelope});
        writeln!(out, "{frame}").unwrap();
    };
    if !request["identity"].as_str().unwrap().ends_with("alice-sub") {
        let view = ErrorView {
            kind: ErrorKind::Refused,
            message: format!("Box {BOX} is not yours"),
            hints: vec![],
            context: vec![],
            detail: None,
        };
        return reply(&mut out, Response::err("attach", view));
    }
    write_bytes(&mut out, &id, b"claude> ").unwrap();
    loop {
        line.clear();
        if read.read_line(&mut line).unwrap_or(0) == 0 {
            seen.lock().unwrap().push(json!("detached"));
            return reply(
                &mut out,
                Response::ok("attach", Some(json!({"ended_by": "client"}))),
            );
        }
        match parse_frame(line.trim_end()).unwrap() {
            ClientFrame::Bytes { len, .. } => {
                let mut keys = vec![0; len];
                read.read_exact(&mut keys).unwrap();
                write_bytes(&mut out, &id, &keys).unwrap();
            }
            ClientFrame::Resize { cols, rows, .. } => {
                write_bytes(&mut out, &id, format!("[{cols}x{rows}]").as_bytes()).unwrap();
            }
            _ => return,
        }
    }
}

fn world() -> World {
    let issuer = Issuer::start(CLIENT_ID);
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let kept = Arc::clone(&kept);
            std::thread::spawn(move || serve(&stream, &kept));
        }
    });
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = tcp.local_addr().unwrap().to_string();
    let login = Arc::new(Login::new(&format!("http://{addr}")).unwrap());
    let gate = Arc::new(Gate {
        controller: moor_client::Client::new(socket),
        mcp: None,
        verifier: Verifier::new(issuer.base.clone(), CLIENT_ID.to_owned(), None),
    });
    std::thread::spawn(move || moor_gate::serve_with(&tcp, &gate, Some(&login)));
    World {
        addr,
        issuer,
        seen,
        _dir: dir,
    }
}

fn csrf(w: &World, cookie: &str) -> String {
    login::csrf(&get(&w.addr, "/auth/me", Some(cookie)))
}

type Socket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

/// Opens the terminal with `cookie` and `origin`; the socket, or the refusal's status.
fn open(w: &World, cookie: &str, origin: &str, csrf: &str) -> Result<Socket, u16> {
    let url = format!(
        "ws://{}/account/api/box/terminal?box={BOX}&cols=80&rows=24&csrf={csrf}",
        w.addr
    );
    let mut req = url.into_client_request().unwrap();
    req.headers_mut().insert("Cookie", cookie.parse().unwrap());
    req.headers_mut().insert("Origin", origin.parse().unwrap());
    match tungstenite::connect(req) {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(res)) => Err(res.status().as_u16()),
        Err(e) => panic!("{e}"),
    }
}

/// Binary messages until the screen holds `want`.
fn screen_until(ws: &mut Socket, want: &str) -> String {
    let mut screen = String::new();
    while !screen.contains(want) {
        match ws.read().unwrap() {
            Message::Binary(b) => screen.push_str(&String::from_utf8_lossy(&b)),
            other => panic!("{other:?} before {want:?} in {screen:?}"),
        }
    }
    screen
}

#[test]
fn a_signed_in_page_types_into_its_box_and_resizes_it() {
    let w = world();
    let cookie = sign_in(&w.addr);
    let origin = format!("http://{}", w.addr);
    let mut ws = open(&w, &cookie, &origin, &csrf(&w, &cookie)).unwrap();
    screen_until(&mut ws, "claude> ");
    ws.send(Message::binary(b"hi\r".to_vec())).unwrap();
    screen_until(&mut ws, "hi\r");
    ws.send(Message::text(r#"{"cols":120,"rows":40}"#)).unwrap();
    screen_until(&mut ws, "[120x40]");
    let attach = w.seen.lock().unwrap()[0].clone();
    assert_eq!(attach["cmd"], "as", "{attach}");
    assert!(
        attach["identity"].as_str().unwrap().ends_with("alice-sub"),
        "{attach}"
    );
    let inner = &attach["request"];
    assert_eq!(
        (inner["cmd"].as_str(), inner["box"].as_str()),
        (Some("attach"), Some(BOX))
    );
    assert_eq!(
        (inner["cols"].as_u64(), inner["rows"].as_u64()),
        (Some(80), Some(24))
    );
    ws.close(None).unwrap();
    for _ in 0..200 {
        if w.seen.lock().unwrap().len() == 2 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!(
        "closing the page did not detach: {:?}",
        w.seen.lock().unwrap()
    );
}

#[test]
fn another_persons_box_ends_with_the_controllers_refusal() {
    let w = world();
    let claims = json!({"sub": "mallory", "email": "mallory@example.com", "email_verified": true});
    *w.issuer.id_claims.lock().unwrap() = claims.clone();
    w.issuer.userinfo.lock().unwrap().1 = claims;
    let cookie = sign_in(&w.addr);
    let origin = format!("http://{}", w.addr);
    let mut ws = open(&w, &cookie, &origin, &csrf(&w, &cookie)).unwrap();
    let Message::Text(ended) = ws.read().unwrap() else {
        panic!("no reason");
    };
    assert!(ended.contains("is not yours"), "{ended}");
}

#[test]
fn another_origin_a_missing_token_or_no_session_is_refused_before_any_attach() {
    let w = world();
    let cookie = sign_in(&w.addr);
    let token = csrf(&w, &cookie);
    let origin = format!("http://{}", w.addr);
    assert_eq!(
        open(&w, &cookie, "https://evil.example", &token).err(),
        Some(403)
    );
    assert_eq!(open(&w, &cookie, &origin, "forged").err(), Some(403));
    assert_eq!(
        open(&w, "moor_session=stale", &origin, &token).err(),
        Some(401)
    );
    assert!(w.seen.lock().unwrap().is_empty());
}
