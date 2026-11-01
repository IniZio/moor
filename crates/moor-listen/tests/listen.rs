//! The `moor-listen` binary against the real gate and a fake controller that serves watches
//! and Box reads from state the test edits.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::disallowed_methods,
    clippy::print_stderr,
    clippy::option_if_let_else,
    clippy::significant_drop_tightening,
    clippy::needless_pass_by_value,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use moor_gate::{Gate, Verifier};
use moor_testkit::oidc::{now, Issuer};
use serde_json::{json, Value};

const CLIENT_ID: &str = "moor-test-client";

/// What the fake controller holds and saw.
#[derive(Default)]
struct Fake {
    /// Names of the open Boxes.
    boxes: Vec<String>,
    /// Each Box's events, oldest first.
    events: HashMap<String, Vec<Value>>,
    /// What each Box last said.
    said: HashMap<String, String>,
    /// Live watches.
    live: Vec<UnixStream>,
}

/// A gate over a fake controller, and the files the token command uses.
struct World {
    gate: String,
    token: String,
    fake: Arc<Mutex<Fake>>,
    dir: tempfile::TempDir,
}

fn answer(stream: UnixStream, fake: &Mutex<Fake>) {
    let mut reply = stream.try_clone().unwrap();
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        let frame: Value = serde_json::from_str(&line).unwrap();
        match frame["frame"].as_str().unwrap() {
            "hello" => writeln!(reply, "{line}").unwrap(),
            "watch" if frame["follow"] != false => {
                fake.lock().unwrap().live.push(reply.try_clone().unwrap());
            }
            "watch" => {
                let up = moor_core_frame();
                reply.write_all(up.as_bytes()).unwrap();
                return;
            }
            _ => {
                let inner = &frame["request"]["request"];
                let f = fake.lock().unwrap();
                let value = match inner["box"].as_str() {
                    Some(name) => json!({
                        "box": {"name": name, "lifecycle": "ready",
                            "said": {"text": f.said.get(name), "at": "2026-11-01T00:00:00Z"}},
                        "events": f.events.get(name).cloned().unwrap_or_default(),
                    }),
                    None => json!({"boxes": f.boxes.iter()
                        .map(|n| json!({"name": n, "lifecycle": "ready"})).collect::<Vec<_>>()}),
                };
                let envelope = moor_core::response::Response::ok("x", Some(value));
                let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                writeln!(reply, "{out}").unwrap();
            }
        }
    }
}

/// The controller's "caught up" frame.
fn moor_core_frame() -> String {
    moor_core::controller::ServerFrame::CaughtUp { cursor: 10 }.to_line()
}

fn world(boxes: &[&str]) -> World {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("c.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let fake = Arc::new(Mutex::new(Fake {
        boxes: boxes.iter().map(|b| (*b).to_owned()).collect(),
        ..Fake::default()
    }));
    let shared = Arc::clone(&fake);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let fake = Arc::clone(&shared);
            std::thread::spawn(move || answer(stream, &fake));
        }
    });
    let Issuer {
        base: issuer, key, ..
    } = Issuer::start(CLIENT_ID);
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let gate = tcp.local_addr().unwrap().to_string();
    let url = format!("http://{gate}/mcp");
    let mcp = moor_gate::Mcp::new(&url, socket.clone()).unwrap();
    let config = Arc::new(Gate {
        front_socket: socket,
        mcp: Some(mcp),
        verifier: Verifier::new(issuer.clone(), CLIENT_ID.to_owned(), None),
    });
    std::thread::spawn(move || moor_gate::serve(&tcp, &config));
    let claims = json!({"iss": issuer, "aud": url, "sub": "alice-sub", "scope": "moor:read",
        "email": "Alice@Example.com", "email_verified": true, "exp": now() + 600});
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("k1".to_owned());
    let token = encode(&header, &claims, &EncodingKey::from_ec_der(&key)).unwrap();
    World {
        gate,
        token,
        fake,
        dir,
    }
}

impl World {
    /// Sets Box `name`'s newest event and what it said, and sends the event to the watches.
    fn happen(&self, name: &str, id: i64, kind: &str, said: &str) {
        {
            let mut f = self.fake.lock().unwrap();
            f.events
                .entry(name.to_owned())
                .or_default()
                .push(json!({"id": id, "kind": kind, "payload": {}, "ts": "t"}));
            f.said.insert(name.to_owned(), said.to_owned());
        }
        self.emit(id, Some(name), kind);
    }

    /// Sends an event to every live watch.
    fn emit(&self, cursor: i64, name: Option<&str>, kind: &str) {
        let frame = moor_core::controller::ServerFrame::Event {
            cursor,
            r#box: name.map(str::to_owned),
            box_id: Some(moor_core::BoxId::new(cursor)),
            session: None,
            kind: kind.to_owned(),
            payload: json!({}),
            ts: 0,
        };
        for live in &mut self.fake.lock().unwrap().live {
            let _ = live.write_all(frame.to_line().as_bytes());
        }
    }

    /// Waits until `n` watches have been opened.
    fn wait_live(&self, n: usize) {
        for _ in 0..1000 {
            if self.fake.lock().unwrap().live.len() >= n {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the gate never opened {n} live watches");
    }

    /// The token command: counts its runs in `runs`, and gives a bad token while `bad` exists.
    fn token_cmd(&self) -> String {
        let script = self.dir.path().join("token.sh");
        let body = format!(
            "#!/bin/sh\necho run >> {d}/runs\nif [ -e {d}/bad ]; then printf bad; else printf %s {t}; fi\n",
            d = self.dir.path().display(),
            t = self.token
        );
        std::fs::write(&script, body).unwrap();
        format!("sh {}", script.display())
    }

    /// How often the token command has run.
    fn runs(&self) -> usize {
        std::fs::read_to_string(self.dir.path().join("runs")).map_or(0, |s| s.lines().count())
    }

    /// Starts `moor-listen` on this gate.
    fn listen(&self) -> Listen {
        let mut child = Command::new(env!("CARGO_BIN_EXE_moor-listen"))
            .args(["--url", &format!("http://{}/mcp", self.gate)])
            .args(["--token-cmd", &self.token_cmd()])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        Listen { child, lines }
    }
}

/// A running `moor-listen` and the lines it printed.
struct Listen {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Listen {
    /// The next line; fails the test after 20 s.
    fn line(&self) -> String {
        self.lines.recv_timeout(Duration::from_secs(20)).unwrap()
    }
}

impl Drop for Listen {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn idled_prints_the_line_with_the_first_line_of_agent_said() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    world.happen("repo/a", 1, "idled", "Opened the PR.\nSecond line.");
    assert_eq!(listen.line(), "moor: box repo/a idled: Opened the PR.");
}

#[test]
fn busy_prints_nothing() {
    let world = world(&["repo/a", "repo/b"]);
    let listen = world.listen();
    world.wait_live(1);
    world.happen("repo/a", 1, "busy", "working");
    world.happen("repo/b", 2, "needs_input", "");
    assert_eq!(listen.line(), "moor: box repo/b needs_input");
}

#[test]
fn a_box_started_after_open_is_picked_up_through_list_changed() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    world.fake.lock().unwrap().boxes.push("repo/c".to_owned());
    world.emit(1, Some("repo/c"), "started");
    world.wait_live(2);
    world.happen("repo/c", 2, "idled", "Done.");
    assert_eq!(listen.line(), "moor: box repo/c idled: Done.");
}

#[test]
fn a_dropped_stream_prints_lost_then_reconnected() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    for live in world.fake.lock().unwrap().live.drain(..) {
        live.shutdown(std::net::Shutdown::Both).unwrap();
    }
    let lost = listen.line();
    assert!(
        lost.starts_with("moor-listen: stream lost (") && lost.ends_with("); reconnecting"),
        "{lost}"
    );
    assert_eq!(listen.line(), "moor-listen: reconnected");
    world.wait_live(1);
    world.happen("repo/a", 1, "idled", "Back.");
    assert_eq!(listen.line(), "moor: box repo/a idled: Back.");
}

#[test]
fn a_rejected_token_prints_the_token_line() {
    let world = world(&["repo/a"]);
    std::fs::write(world.dir.path().join("bad"), "").unwrap();
    let listen = world.listen();
    assert_eq!(
        listen.line(),
        "moor-listen: token rejected; call any moor tool so the client refreshes it"
    );
}

#[test]
fn the_token_command_reruns_on_reconnect() {
    let world = world(&["repo/a"]);
    let listen = world.listen();
    world.wait_live(1);
    let before = world.runs();
    assert!(before >= 1);
    for live in world.fake.lock().unwrap().live.drain(..) {
        live.shutdown(std::net::Shutdown::Both).unwrap();
    }
    listen.line();
    assert_eq!(listen.line(), "moor-listen: reconnected");
    assert!(world.runs() > before, "{} runs", world.runs());
}
