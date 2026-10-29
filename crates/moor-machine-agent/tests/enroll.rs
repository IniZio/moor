//! `enroll` against the testkit issuer and an in-process gate, in front of a controller that
//! answers every `machine-enroll` as the real one shapes it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

use moor_client::oidc::Ui;
use moor_core::response::Response;
use moor_gate::{Gate, Verifier};
use moor_machine_agent::{enroll, Enroll, State};
use moor_testkit::oidc::Issuer;
use serde_json::{json, Value};

const CLIENT_ID: &str = "moor-test-client";

struct World {
    gate: String,
    issuer: Issuer,
    seen: mpsc::Receiver<Value>,
    _dir: tempfile::TempDir,
}

fn serve_controller(path: &Path) -> mpsc::Receiver<Value> {
    let listener = UnixListener::bind(path).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reply = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let frame: Value = serde_json::from_str(&line).unwrap();
                if frame["frame"] == "hello" {
                    writeln!(reply, "{line}").unwrap();
                    continue;
                }
                let asked = frame["request"]["request"].clone();
                let value = json!({"machine_id": 7, "name": asked["name"], "user": 1,
                    "owner": asked["email"], "capacity": asked["capacity"]});
                tx.send(frame["request"].clone()).unwrap();
                let envelope = Response::ok("machine-enroll", Some(value));
                let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                writeln!(reply, "{out}").unwrap();
            }
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
    seen: mpsc::Receiver<Value>,
    dir: tempfile::TempDir,
) -> World {
    let issuer = Issuer::start(CLIENT_ID);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let gate = format!("http://{}", listener.local_addr().unwrap());
    let config = Arc::new(Gate {
        front_socket: socket,
        mcp: None,
        verifier: Verifier::new(issuer.base.clone(), CLIENT_ID.to_owned(), None),
    });
    std::thread::spawn(move || moor_gate::serve(&listener, &config));
    World {
        gate,
        issuer,
        seen,
        _dir: dir,
    }
}

/// A person at a browser: approves at once, and either the browser reaches the loopback or
/// they paste the address it ended on.
struct Person {
    via_loopback: bool,
    paste: Option<Sender<String>>,
}

impl Ui for Person {
    fn say(&mut self, _line: &str) {}

    fn open(&mut self, url: &str) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into();
        let res = agent.get(url).call().unwrap();
        let back = res.headers()["location"].to_str().unwrap().to_owned();
        if self.via_loopback {
            agent.get(&back).call().unwrap();
        } else {
            self.paste.take().unwrap().send(back).unwrap();
        }
    }

    fn pasted(&mut self, to: Sender<String>) {
        self.paste = Some(to);
    }
}

fn request(w: &World, new_key: bool) -> Enroll {
    Enroll {
        server: w.gate.clone(),
        name: "minion".to_owned(),
        labels: vec!["macos".to_owned()],
        slots: 2,
        new_key,
    }
}

fn run(w: &World, dir: &Path, via_loopback: bool, new_key: bool) -> Result<State, String> {
    let mut person = Person {
        via_loopback,
        paste: None,
    };
    enroll(&request(w, new_key), dir, 0, &mut person)
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn public_key(dir: &Path) -> String {
    let private = std::fs::read_to_string(dir.join("id_ed25519")).unwrap();
    let key = ssh_key::PrivateKey::from_openssh(private).unwrap();
    key.public_key().to_openssh().unwrap()
}

#[test]
fn enroll_signs_in_posts_the_key_and_saves_the_controllers_answer() {
    let w = world();
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("moor-machine-agent");
    let state = run(&w, &dir, true, false).unwrap();
    let sent = w.seen.try_recv().unwrap();
    let expected = json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
        "cmd": "machine-enroll", "email": "alice@example.com", "name": "minion",
        "pubkey": public_key(&dir), "labels": ["macos"], "capacity": 2}});
    assert_eq!(
        sent, expected,
        "the key on disk was enrolled as the signed-in person"
    );
    let expected = State {
        server: w.gate,
        machine_id: 7,
        owner: "alice@example.com".to_owned(),
        name: "minion".to_owned(),
        slots: 2,
    };
    assert_eq!(state, expected);
    let saved: State =
        serde_json::from_str(&std::fs::read_to_string(dir.join("state.json")).unwrap()).unwrap();
    assert_eq!(saved, expected);
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("id_ed25519")), 0o600);
    assert_eq!(mode(&dir.join("state.json")), 0o600);
}

#[test]
fn a_pasted_callback_address_finishes_the_sign_in() {
    let w = world();
    let home = tempfile::tempdir().unwrap();
    let state = run(&w, home.path(), false, false).unwrap();
    assert_eq!(state.machine_id, 7);
}

#[test]
fn enrolling_again_keeps_the_key_unless_a_new_one_is_asked_for() {
    let w = world();
    let home = tempfile::tempdir().unwrap();
    run(&w, home.path(), true, false).unwrap();
    let first = public_key(home.path());
    run(&w, home.path(), true, false).unwrap();
    assert_eq!(public_key(home.path()), first);
    run(&w, home.path(), true, true).unwrap();
    let fresh = public_key(home.path());
    assert_ne!(fresh, first);
    let sent: Vec<Value> = w.seen.try_iter().collect();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[1]["request"]["pubkey"], first);
    assert_eq!(sent[2]["request"]["pubkey"], fresh);
}

#[test]
fn an_unverified_email_is_refused_and_saves_no_state() {
    let w = world();
    w.issuer.id_claims.lock().unwrap()["email_verified"] = json!(false);
    let home = tempfile::tempdir().unwrap();
    let err = run(&w, home.path(), true, false).unwrap_err();
    assert!(err.contains("401") && err.contains("not verified"), "{err}");
    assert!(!home.path().join("state.json").exists());
    assert!(w.seen.try_recv().is_err(), "nothing reached the controller");
}

/// Runs only with `MOOR_BIN` naming a built `moor`: a real controller records the machine.
#[test]
#[ignore = "needs MOOR_BIN, a built moor binary"]
fn a_real_controller_records_the_enrolled_machine() {
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
    let socket = root.join("state/fronts/authgear/front");
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let (_tx, seen) = mpsc::channel();
    let w = world_on(socket, seen, tempfile::tempdir().unwrap());
    let agent = root.join("agent");
    let state = run(&w, &agent, true, false);
    controller.kill().unwrap();
    controller.wait().unwrap();
    let state = state.unwrap();
    let db = rusqlite::Connection::open(root.join("state/moor.db")).unwrap();
    let row: (i64, String, String, String, u16) = db
        .query_row(
            "SELECT m.machine_id, u.email, m.pubkey, m.labels, m.capacity \
             FROM user_machines_v2 m JOIN users u ON u.id = m.user WHERE m.name = 'minion'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    let want = (
        state.machine_id,
        "alice@example.com".to_owned(),
        public_key(&agent),
        r#"["macos"]"#.to_owned(),
        2,
    );
    assert_eq!(row, want);
}
