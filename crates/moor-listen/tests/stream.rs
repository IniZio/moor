//! How `moor-listen` treats the listen stream itself (keepalives, silence, drops and the gap a
//! reconnect leaves), against a fake gate whose every listen follows a plan the test sets.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::disallowed_methods,
    clippy::missing_docs_in_private_items,
    clippy::significant_drop_tightening,
    clippy::needless_pass_by_value,
    reason = "tests"
)]

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{sleep, spawn};
use std::time::Duration;

use serde_json::{json, Value};

/// What one listen does after its acknowledgement.
#[derive(Clone, Copy)]
enum Plan {
    /// A keepalive comment every 100 ms until the client goes.
    Keep,
    /// Keepalives for this long, then the gate closes the stream.
    CloseAfter(Duration),
    /// Not a byte more, with the connection held open.
    Silent,
    /// Answers 500 instead of a stream.
    Refuse,
}

#[derive(Default)]
struct Fake {
    /// Each Box's events, oldest first, by name.
    events: HashMap<String, Vec<Value>>,
    /// What each Box last said.
    said: HashMap<String, String>,
    /// Plans for the next listens; `Keep` once they run out.
    plans: VecDeque<Plan>,
    /// Listens asked for so far.
    listens: usize,
    /// Live listen streams, to push notifications down.
    live: Vec<TcpStream>,
}

type Shared = Arc<Mutex<Fake>>;

fn respond(mut out: &TcpStream, status: &str, kind: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    out.write_all(head.as_bytes()).unwrap();
    out.write_all(body.as_bytes()).unwrap();
}

fn result(out: &TcpStream, value: Value) {
    let body = json!({"jsonrpc": "2.0", "id": 1, "result": value}).to_string();
    respond(out, "200 OK", "application/json", &body);
}

fn request(stream: &TcpStream) -> Value {
    let mut reader = BufReader::new(stream);
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end().to_ascii_lowercase();
        if line.is_empty() {
            break;
        }
        if let Some(n) = line.strip_prefix("content-length:") {
            length = n.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn serve(stream: TcpStream, fake: &Shared) {
    let req = request(&stream);
    match req["method"].as_str().unwrap() {
        "resources/list" => {
            let f = fake.lock().unwrap();
            let mut names: Vec<_> = f.events.keys().collect();
            names.sort();
            let list: Vec<_> = names
                .iter()
                .map(|n| json!({"uri": format!("moor://box/{n}")}))
                .collect();
            drop(f);
            result(&stream, json!({"resources": list}));
        }
        "resources/read" => {
            let uri = req["params"]["uri"].as_str().unwrap();
            let name = uri.strip_prefix("moor://box/").unwrap();
            let f = fake.lock().unwrap();
            let status =
                json!({"events": f.events.get(name), "agent_said": {"text": f.said.get(name)}});
            drop(f);
            result(&stream, json!({"contents": [{"text": status.to_string()}]}));
        }
        "subscriptions/listen" => listen(stream, fake),
        other => panic!("unexpected {other}"),
    }
}

fn listen(mut stream: TcpStream, fake: &Shared) {
    let plan = {
        let mut f = fake.lock().unwrap();
        f.listens += 1;
        f.plans.pop_front().unwrap_or(Plan::Keep)
    };
    if matches!(plan, Plan::Refuse) {
        respond(&stream, "500 Internal Server Error", "text/plain", "no");
        return;
    }
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
    let ack = r#"{"jsonrpc":"2.0","method":"notifications/subscriptions/acknowledged"}"#;
    stream
        .write_all(format!("{head}data: {ack}\n\n").as_bytes())
        .unwrap();
    fake.lock().unwrap().live.push(stream.try_clone().unwrap());
    let ticks = match plan {
        Plan::Keep => u32::MAX,
        Plan::CloseAfter(d) => d.as_millis().try_into().unwrap_or(u32::MAX) / 100,
        Plan::Silent => {
            sleep(Duration::from_secs(60));
            return;
        }
        Plan::Refuse => unreachable!(),
    };
    for _ in 0..ticks {
        sleep(Duration::from_millis(100));
        if stream.write_all(b": keepalive\n\n").is_err() {
            return;
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

struct Gate {
    url: String,
    fake: Shared,
}

/// A fake gate with these Boxes, each holding the given events.
fn gate(boxes: &[(&str, &[(i64, &str)])], plans: &[Plan]) -> Gate {
    let mut fake = Fake {
        plans: plans.iter().copied().collect(),
        ..Fake::default()
    };
    for (name, events) in boxes {
        let events = events
            .iter()
            .map(|(id, kind)| json!({"id": id, "kind": kind}))
            .collect();
        fake.events.insert((*name).to_owned(), events);
    }
    let fake = Arc::new(Mutex::new(fake));
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/mcp", tcp.local_addr().unwrap());
    let shared = Arc::clone(&fake);
    spawn(move || {
        for stream in tcp.incoming().flatten() {
            let fake = Arc::clone(&shared);
            spawn(move || serve(stream, &fake));
        }
    });
    Gate { url, fake }
}

impl Gate {
    fn listens(&self) -> usize {
        self.fake.lock().unwrap().listens
    }

    fn wait_listens(&self, n: usize) {
        for _ in 0..1500 {
            if self.listens() >= n {
                return;
            }
            sleep(Duration::from_millis(10));
        }
        panic!("only {} listens, wanted {n}", self.listens());
    }

    /// Box `name` gets a new event, told to no stream.
    fn quietly(&self, name: &str, id: i64, kind: &str, said: &str) {
        let mut f = self.fake.lock().unwrap();
        f.events
            .get_mut(name)
            .unwrap()
            .push(json!({"id": id, "kind": kind}));
        f.said.insert(name.to_owned(), said.to_owned());
    }

    /// Box `name` gets a new event, told to every live stream.
    fn happen(&self, name: &str, id: i64, kind: &str, said: &str) {
        self.quietly(name, id, kind, said);
        let note = json!({"jsonrpc": "2.0", "method": "notifications/resources/updated",
            "params": {"uri": format!("moor://box/{name}")}});
        for live in &mut self.fake.lock().unwrap().live {
            let _ = live.write_all(format!("data: {note}\n\n").as_bytes());
        }
    }

    fn listen(&self, idle_ms: u64) -> Listen {
        let mut child = Command::new(env!("CARGO_BIN_EXE_moor-listen"))
            .args(["--url", &self.url, "--token-cmd", "printf tok"])
            .args(["--idle-ms", &idle_ms.to_string()])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        Listen { child, lines }
    }
}

struct Listen {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Listen {
    fn line(&self) -> String {
        self.lines.recv_timeout(Duration::from_secs(20)).unwrap()
    }

    fn quiet_for(&self, d: Duration) {
        if let Ok(line) = self.lines.recv_timeout(d) {
            panic!("printed {line:?}");
        }
    }
}

impl Drop for Listen {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn keepalives_more_often_than_the_idle_limit_keep_the_stream() {
    let gate = gate(&[("repo/a", &[])], &[Plan::Keep]);
    let listen = gate.listen(300);
    gate.wait_listens(1);
    listen.quiet_for(Duration::from_millis(1200));
    assert_eq!(gate.listens(), 1);
}

#[test]
fn a_silent_stream_is_dropped_and_listened_again() {
    let gate = gate(&[("repo/a", &[])], &[Plan::Silent, Plan::Keep]);
    let listen = gate.listen(300);
    gate.wait_listens(2);
    gate.happen("repo/a", 1, "idled", "Back.");
    assert_eq!(listen.line(), "moor: box repo/a idled: Back.");
}

#[test]
fn a_drop_that_reconnects_at_once_prints_nothing() {
    let gate = gate(
        &[("repo/a", &[])],
        &[Plan::CloseAfter(Duration::from_millis(200)), Plan::Keep],
    );
    let listen = gate.listen(45_000);
    gate.wait_listens(2);
    gate.happen("repo/a", 1, "idled", "Still here.");
    assert_eq!(listen.line(), "moor: box repo/a idled: Still here.");
}

#[test]
fn a_failed_reconnect_prints_lost_then_reconnected() {
    let gate = gate(
        &[("repo/a", &[])],
        &[
            Plan::CloseAfter(Duration::from_millis(200)),
            Plan::Refuse,
            Plan::Keep,
        ],
    );
    let listen = gate.listen(45_000);
    let lost = listen.line();
    assert!(
        lost.starts_with("moor-listen: stream lost (") && lost.ends_with("); reconnecting"),
        "{lost}"
    );
    assert_eq!(listen.line(), "moor-listen: reconnected");
    assert_eq!(gate.listens(), 3);
}

#[test]
fn a_box_that_idled_while_disconnected_is_said_once_after_reconnect() {
    let gate = gate(
        &[("repo/a", &[(1, "busy")]), ("repo/b", &[(2, "idled")])],
        &[Plan::CloseAfter(Duration::from_millis(500)), Plan::Keep],
    );
    let listen = gate.listen(45_000);
    gate.wait_listens(1);
    gate.quietly("repo/a", 3, "idled", "Done.");
    assert_eq!(listen.line(), "moor: box repo/a idled: Done.");
    assert_eq!(gate.listens(), 2);
    listen.quiet_for(Duration::from_secs(1));
}
