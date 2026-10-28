//! The WebSocket transport against a fake `/front`: the same exchanges as the socket tests,
//! with the server's bytes cut into tiny messages.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use moor_core::request::AttachReq;
use tungstenite::handshake::server::{ErrorResponse, Request as Upgrade, Response as Accepted};
use tungstenite::http::Response as Http;
use tungstenite::{Message, WebSocket};

use super::*;

#[derive(Debug)]
struct Fixed(&'static str);

impl TokenSource for Fixed {
    fn bearer(&self) -> Result<String, String> {
        Ok(self.0.to_owned())
    }
}

#[derive(Debug)]
struct Nobody;

impl TokenSource for Nobody {
    fn bearer(&self) -> Result<String, String> {
        Err("run login first".to_owned())
    }
}

/// The server's end of one connection: reads the client's byte stream line by line and
/// writes in three-byte messages.
struct Wire {
    ws: WebSocket<TcpStream>,
    pending: Vec<u8>,
}

impl Wire {
    fn fill(&mut self) -> bool {
        match self.ws.read() {
            Ok(Message::Binary(b)) => {
                self.pending.extend_from_slice(&b);
                true
            }
            _ => false,
        }
    }

    fn line(&mut self) -> String {
        loop {
            if let Some(at) = self.pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=at).collect();
                return String::from_utf8(line).unwrap().trim_end().to_owned();
            }
            assert!(self.fill(), "client closed before a full line");
        }
    }

    fn take(&mut self, n: usize) -> Vec<u8> {
        while self.pending.len() < n {
            assert!(self.fill(), "client closed before {n} bytes");
        }
        self.pending.drain(..n).collect()
    }

    fn say(&mut self, bytes: &[u8]) {
        for part in bytes.chunks(3) {
            self.ws.send(Message::Binary(part.to_vec().into())).unwrap();
        }
    }

    fn sayln(&mut self, line: &str) {
        self.say(format!("{line}\n").as_bytes());
    }

    /// Reads until the client says it will write no more.
    fn until_half_close(&mut self) {
        while self
            .ws
            .read()
            .is_ok_and(|m| !m.is_close() && !matches!(&m, Message::Binary(b) if b.is_empty()))
        {
        }
    }

    /// Tells the client the server will write no more.
    fn half_close(&mut self) {
        self.ws.send(Message::Binary(Vec::new().into())).unwrap();
    }

    fn close(&mut self) {
        self.ws.close(None).unwrap();
        while self.ws.read().is_ok() {}
    }
}

#[allow(
    clippy::result_large_err,
    reason = "tungstenite's callback type fixes the error response"
)]
/// Serves one connection on loopback. Returns the server URL and the `Authorization`
/// header the upgrade carried.
fn serve_ws(
    script: impl FnOnce(&mut Wire) + Send + 'static,
) -> (String, JoinHandle<Option<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let seen = Arc::new(Mutex::new(None));
        let keep = Arc::clone(&seen);
        let check = move |req: &Upgrade, resp: Accepted| -> Result<Accepted, ErrorResponse> {
            let auth = req
                .headers()
                .get("authorization")
                .map(|v| v.to_str().unwrap().to_owned());
            assert_eq!(req.uri().path(), "/front");
            *keep.lock().unwrap() = auth.clone();
            if auth.as_deref() == Some("Bearer good") {
                Ok(resp)
            } else {
                Err(Http::builder().status(401).body(None).unwrap())
            }
        };
        if let Ok(ws) = tungstenite::accept_hdr(tcp, check) {
            script(&mut Wire {
                ws,
                pending: Vec::new(),
            });
        }
        let auth = seen.lock().unwrap().take();
        auth
    });
    (url, handle)
}

fn remote(url: &str) -> Client {
    Client::remote(url, Arc::new(Fixed("good")))
}

#[test]
fn a_request_runs_over_the_front_with_the_bearer_token() {
    let (url, server) = serve_ws(|wire| {
        assert_eq!(wire.line(), hello());
        wire.sayln(&hello());
        assert!(wire.line().starts_with(r#"{"frame":"request","id":"1""#));
        wire.sayln(r#"{"frame":"progress","id":"1","message":"one"}"#);
        wire.sayln(&format!(
            r#"{{"frame":"response","id":"1","envelope":{}}}"#,
            ok_envelope()
        ));
        wire.until_half_close();
        wire.close();
    });
    let mut notes = Vec::new();
    let resp = remote(&url)
        .request(show(), |m| notes.push(m.to_owned()))
        .unwrap();
    assert_eq!(resp, Response::ok("show", None));
    assert_eq!(notes, ["one"]);
    assert_eq!(server.join().unwrap().as_deref(), Some("Bearer good"));
}

#[test]
fn raw_bytes_cross_message_boundaries() {
    let payload: Vec<u8> = (0..=255).cycle().take(5000).collect();
    let expect = payload.clone();
    let (url, server) = serve_ws(move |wire| {
        wire.line();
        wire.sayln(&hello());
        wire.line();
        wire.say(&bytes_frame(&payload[..2000]));
        wire.say(&bytes_frame(&payload[2000..]));
        wire.say(&response_line());
        wire.until_half_close();
        wire.close();
    });
    let mut stream = remote(&url).read_stream(show()).unwrap();
    let mut got = Vec::new();
    stream.read_to_end(&mut got).unwrap();
    assert_eq!(got, expect);
    assert_eq!(stream.finish().unwrap(), Response::ok("show", None));
    server.join().unwrap();
}

#[test]
fn attach_carries_keys_out_and_the_screen_back_until_detach() {
    let (url, server) = serve_ws(|wire| {
        wire.line();
        wire.sayln(&hello());
        assert!(wire.line().contains(r#""cmd":"attach""#));
        wire.say(&bytes_frame(b"screen"));
        let head = wire.line();
        assert_eq!(head, r#"{"frame":"bytes","id":"1","len":4}"#);
        assert_eq!(wire.take(4), b"keys");
        wire.say(&bytes_frame(b"echo:keys"));
        wire.until_half_close();
        wire.close();
    });
    let req: AttachReq =
        serde_json::from_value(serde_json::json!({"box": "fix", "cols": 80, "rows": 24})).unwrap();
    let (mut screen, mut keys) = remote(&url).attach(req).unwrap();
    let mut seen = vec![0; 6];
    screen.read_exact(&mut seen).unwrap();
    assert_eq!(seen, b"screen");
    keys.send(b"keys").unwrap();
    let mut echoed = vec![0; 9];
    screen.read_exact(&mut echoed).unwrap();
    assert_eq!(echoed, b"echo:keys");
    keys.detach().unwrap();
    server.join().unwrap();
}

#[test]
fn a_refused_token_is_unauthorized_and_not_redialed() {
    let (url, server) = serve_ws(|_| {});
    let client = Client::remote(&url, Arc::new(Fixed("stale")));
    let err = client.request(show(), |_| {}).unwrap_err();
    assert!(matches!(err, ClientError::Unauthorized(_)), "{err}");
    assert_eq!(server.join().unwrap().as_deref(), Some("Bearer stale"));
}

#[test]
fn no_token_fails_before_any_dial() {
    let err = Client::remote("https://example.com", Arc::new(Nobody))
        .request(show(), |_| {})
        .unwrap_err();
    assert!(
        matches!(&err, ClientError::Unauthorized(m) if m.contains("login")),
        "{err}"
    );
}

#[test]
fn a_token_is_never_sent_in_clear_to_a_remote_host() {
    let err = Client::remote("http://example.com", Arc::new(Fixed("good")))
        .request(show(), |_| {})
        .unwrap_err();
    assert!(
        matches!(&err, ClientError::Remote(m) if m.contains("unencrypted")),
        "{err}"
    );
}

#[test]
fn a_client_half_close_leaves_the_server_talking_until_close() {
    let (url, server) = serve_ws(|wire| {
        wire.line();
        wire.sayln(&hello());
        wire.line();
        wire.until_half_close();
        wire.say(&bytes_frame(b"after detach"));
        wire.say(&response_line());
        wire.close();
    });
    let req: AttachReq =
        serde_json::from_value(serde_json::json!({"box": "fix", "cols": 80, "rows": 24})).unwrap();
    let (mut screen, keys) = remote(&url).attach(req).unwrap();
    keys.detach().unwrap();
    let mut late = vec![0; 12];
    screen.read_exact(&mut late).unwrap();
    assert_eq!(late, b"after detach");
    assert_eq!(screen.read(&mut [0; 1]).unwrap(), 0);
    assert_eq!(screen.finish().unwrap(), Response::ok("show", None));
    server.join().unwrap();
}

#[test]
fn a_server_half_close_ends_the_reads_but_not_the_writes() {
    let (url, server) = serve_ws(|wire| {
        wire.line();
        wire.sayln(&hello());
        wire.line();
        wire.say(&bytes_frame(b"screen"));
        wire.say(&response_line());
        wire.half_close();
        assert_eq!(wire.line(), r#"{"frame":"bytes","id":"1","len":4}"#);
        assert_eq!(wire.take(4), b"keys");
        wire.until_half_close();
        wire.close();
    });
    let req: AttachReq =
        serde_json::from_value(serde_json::json!({"box": "fix", "cols": 80, "rows": 24})).unwrap();
    let (mut screen, mut keys) = remote(&url).attach(req).unwrap();
    let mut seen = vec![0; 6];
    screen.read_exact(&mut seen).unwrap();
    assert_eq!(seen, b"screen");
    assert_eq!(screen.read(&mut [0; 1]).unwrap(), 0);
    keys.send(b"keys").unwrap();
    keys.detach().unwrap();
    server.join().unwrap();
}
