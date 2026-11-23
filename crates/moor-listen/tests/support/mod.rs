//! A gate over a fake controller that serves watches, Box reads and workflow run changes from
//! state the test edits, and `moor-listen` started on it.
#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test file uses its own part of the harness"
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use moor_gate::{Gate, Verifier};
use moor_testkit::oidc::{now, Issuer};
use moor_testkit::ChildGuard;
use serde_json::{json, Value};

pub const CLIENT_ID: &str = "moor-test-client";

/// What the fake controller holds and saw.
#[derive(Default)]
pub struct Fake {
    /// Names of the open Boxes.
    pub boxes: Vec<String>,
    /// Each Box's events, oldest first.
    pub events: HashMap<String, Vec<Value>>,
    /// What each Box last said.
    pub said: HashMap<String, String>,
    /// Live watches.
    pub live: Vec<UnixStream>,
    /// Workflow run changes, oldest first, each with its `seq`.
    pub runs: Vec<Value>,
}

/// A gate over a fake controller, and the files the token command uses.
pub struct World {
    pub gate: String,
    pub token: String,
    pub fake: Arc<Mutex<Fake>>,
    pub dir: tempfile::TempDir,
    /// The OIDC issuer a saved sign-in refreshes against.
    pub issuer: Issuer,
}

pub fn answer(stream: UnixStream, fake: &Mutex<Fake>) {
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
                let value = if inner["cmd"] == "workflow-runs" {
                    runs_after(&f.runs, inner)
                } else if inner["cmd"] == "workflow-run-list" {
                    let run = f.runs.iter().rfind(|c| c["id"] == inner["run"]).cloned();
                    json!({"workflow": inner["workflow"], "runs": run.into_iter().collect::<Vec<_>>()})
                } else {
                    match inner["box"].as_str() {
                        Some(name) => json!({
                            "box": {"name": name, "lifecycle": "ready",
                                "said": {"text": f.said.get(name), "at": "2026-11-01T00:00:00Z"}},
                            "events": f.events.get(name).cloned().unwrap_or_default(),
                        }),
                        None => json!({"boxes": f.boxes.iter()
                        .map(|n| json!({"name": n, "lifecycle": "ready"})).collect::<Vec<_>>()}),
                    }
                };
                let envelope = moor_core::response::Response::ok("x", Some(value));
                let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                writeln!(reply, "{out}").unwrap();
            }
        }
    }
}

/// What `workflow-runs` answers from `runs` for `req`: its workflow's changes after its cursor,
/// at most its limit.
pub fn runs_after(runs: &[Value], req: &Value) -> Value {
    let after = req["after"].as_i64().unwrap_or(0);
    let limit = req["limit"].as_u64().unwrap_or(50);
    let changes: Vec<Value> = runs
        .iter()
        .filter(|c| c["seq"].as_i64().unwrap() > after)
        .filter(|c| req["workflow"].is_null() || c["workflow"] == req["workflow"])
        .take(usize::try_from(limit).unwrap())
        .cloned()
        .collect();
    let next = changes
        .iter()
        .filter_map(|c| c["seq"].as_i64())
        .max()
        .unwrap_or(after);
    json!({"changes": changes, "next": next})
}

/// The controller's "caught up" frame.
pub fn moor_core_frame() -> String {
    moor_core::controller::ServerFrame::CaughtUp { cursor: 10 }.to_line()
}

pub fn world(boxes: &[&str]) -> World {
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
    let oidc = Issuer::start(CLIENT_ID);
    let (issuer, key) = (oidc.base.clone(), oidc.key.clone());
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let gate = tcp.local_addr().unwrap().to_string();
    let url = format!("http://{gate}/mcp");
    let mcp = moor_gate::Mcp::new(&url, moor_client::Client::new(&socket)).unwrap();
    let config = Arc::new(Gate {
        controller: moor_client::Client::new(socket),
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
        issuer: oidc,
    }
}

impl World {
    /// Sets Box `name`'s newest event and what it said, and sends the event to the watches.
    pub fn happen(&self, name: &str, id: i64, kind: &str, said: &str) {
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

    /// Like `happen`, for an event that carries `payload` and no agent message.
    pub fn happen_with(&self, name: &str, id: i64, kind: &str, payload: Value) {
        {
            let mut f = self.fake.lock().unwrap();
            f.events
                .entry(name.to_owned())
                .or_default()
                .push(json!({"id": id, "kind": kind, "payload": payload, "ts": "t"}));
            f.said.insert(name.to_owned(), String::new());
        }
        self.emit(id, Some(name), kind);
    }

    /// Sends an event to every live watch.
    pub fn emit(&self, cursor: i64, name: Option<&str>, kind: &str) {
        self.emit_with(cursor, name, kind, json!({}));
    }

    /// Records the change of a workflow run, whose `seq` it sets, and sends the `run_moved`
    /// its move writes on Box `on` to every live watch.
    pub fn move_run(&self, on: &str, mut change: Value) {
        let seq = {
            let mut f = self.fake.lock().unwrap();
            let seq = i64::try_from(f.runs.len()).unwrap() + 1;
            change["seq"] = json!(seq);
            f.runs.push(change.clone());
            seq
        };
        let payload = json!({"workflow": change["workflow"], "run": change["id"]});
        self.emit_with(1000 + seq, Some(on), "run_moved", payload);
    }

    /// Records a run change that no watch hears of, as one made before a listen started.
    pub fn moved_before(&self, mut change: Value) {
        let mut f = self.fake.lock().unwrap();
        change["seq"] = json!(f.runs.len() + 1);
        f.runs.push(change);
    }

    /// Sends an event with `payload` to every live watch.
    pub fn emit_with(&self, cursor: i64, name: Option<&str>, kind: &str, payload: Value) {
        let frame = moor_core::controller::ServerFrame::Event {
            cursor,
            r#box: name.map(str::to_owned),
            box_id: Some(moor_core::BoxId::new(cursor)),
            session: None,
            kind: kind.to_owned(),
            payload,
            ts: 0,
        };
        for live in &mut self.fake.lock().unwrap().live {
            let _ = live.write_all(frame.to_line().as_bytes());
        }
    }

    /// Waits until `n` watches have been opened.
    pub fn wait_live(&self, n: usize) {
        for _ in 0..1000 {
            if self.fake.lock().unwrap().live.len() >= n {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the gate never opened {n} live watches");
    }

    /// The token command: counts its runs in `runs`, gives a bad token while `bad` exists and
    /// fails while `down` exists.
    pub fn token_cmd(&self) -> String {
        let script = self.dir.path().join("token.sh");
        let body = format!(
            "#!/bin/sh\necho run >> {d}/runs\n[ -e {d}/down ] && exit 1\nif [ -e {d}/bad ]; then printf bad; else printf %s {t}; fi\n",
            d = self.dir.path().display(),
            t = self.token
        );
        std::fs::write(&script, body).unwrap();
        format!("sh {}", script.display())
    }

    /// How often the token command has run.
    pub fn runs(&self) -> usize {
        std::fs::read_to_string(self.dir.path().join("runs")).map_or(0, |s| s.lines().count())
    }

    /// Drops every live watch with the token command failing, so the first reconnect fails.
    pub fn drop_watches(&self) {
        std::fs::write(self.dir.path().join("down"), "").unwrap();
        for live in self.fake.lock().unwrap().live.drain(..) {
            live.shutdown(std::net::Shutdown::Both).unwrap();
        }
    }

    /// Lets the token command work again.
    pub fn come_back(&self) {
        std::fs::remove_file(self.dir.path().join("down")).unwrap();
    }

    /// Starts `moor-listen` on this gate.
    pub fn listen(&self) -> Listen {
        self.listen_only(&[])
    }

    /// Starts `moor-listen` on this gate, watching only the Boxes in `only`.
    pub fn listen_only(&self, only: &[&str]) -> Listen {
        let args: Vec<&str> = only.iter().flat_map(|name| ["--box", name]).collect();
        self.listen_with(&args)
    }

    /// Starts `moor-listen` on this gate with `args` after its URL and token command.
    pub fn listen_with(&self, args: &[&str]) -> Listen {
        let mut child = ChildGuard::new(
            Command::new(env!("CARGO_BIN_EXE_moor-listen"))
                .args(["--url", &format!("http://{}/mcp", self.gate)])
                .args(["--token-cmd", &self.token_cmd()])
                .args(args)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
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
pub struct Listen {
    pub child: ChildGuard,
    pub lines: mpsc::Receiver<String>,
}

impl Listen {
    /// The next line; fails the test after 20 s.
    pub fn line(&self) -> String {
        self.lines.recv_timeout(Duration::from_secs(20)).unwrap()
    }
}

impl Drop for Listen {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
