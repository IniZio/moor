//! `run` against an in-process runner taking links, in front of a controller that lists one
//! enrolled machine's key and keeps the last `machine-links` it was sent.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    clippy::disallowed_methods,
    clippy::panic,
    clippy::missing_const_for_fn,
    reason = "tests"
)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moor_client::link::{self, Frame};
use moor_core::response::Response;
use moor_machine_agent::{run, State};
use serde_json::{json, Value};
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey};

const MACHINE: i64 = 7;

struct World {
    server: String,
    key: PrivateKey,
    links: Arc<Mutex<Option<Value>>>,
    dir: tempfile::TempDir,
}

fn serve_controller(path: &Path, pubkey: String, links: Arc<Mutex<Option<Value>>>) {
    let listener = UnixListener::bind(path).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (pubkey, links) = (pubkey.clone(), Arc::clone(&links));
            std::thread::spawn(move || {
                let mut reply = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let frame: Value = serde_json::from_str(&line).unwrap();
                    if frame["frame"] == "hello" {
                        writeln!(reply, "{line}").unwrap();
                        continue;
                    }
                    let cmd = frame["request"]["cmd"].as_str().unwrap().to_owned();
                    let value = if cmd == "machine-keys" {
                        json!({"machines": [{"machine_id": MACHINE, "user": 1, "name": "minion",
                            "pubkey": pubkey}]})
                    } else {
                        let sent = frame["request"]["links"].clone();
                        *links.lock().unwrap() = Some(sent.clone());
                        json!({"links": sent})
                    };
                    let envelope = Response::ok(&cmd, Some(value));
                    let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                    writeln!(reply, "{out}").unwrap();
                }
            });
        }
    });
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    let links = Arc::new(Mutex::new(None));
    let socket = dir.path().join("controller.sock");
    let pubkey = key.public_key().to_openssh().unwrap();
    serve_controller(&socket, pubkey, Arc::clone(&links));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { moor_machine_runner::serve_agents(listener, &socket).await });
    World {
        server,
        key,
        links,
        dir,
    }
}

/// An agent state dir for machine `machine_id` holding `key`.
fn agent_dir(w: &World, machine_id: i64, key: &PrivateKey) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let state = State {
        server: w.server.clone(),
        machine_id,
        owner: "ada@example.com".to_owned(),
        name: "minion".to_owned(),
        slots: 1,
    };
    std::fs::write(
        dir.path().join("state.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();
    let text = key.to_openssh(LineEnding::LF).unwrap();
    std::fs::write(dir.path().join("id_ed25519"), text.as_bytes()).unwrap();
    dir
}

fn spawn_agent(dir: &Path) -> tokio::task::JoinHandle<Result<(), String>> {
    let dir = dir.to_owned();
    tokio::spawn(async move { run(&dir, &|_| {}).await })
}

/// Waits for the controller to be told machine 7 is `online`.
async fn reported(w: &World, online: bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let links = w.links.lock().unwrap().clone();
        if let Some(links) = &links {
            if links[0]["machine_id"] == MACHINE && links[0]["online"] == online {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "never reported online={online}: {links:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Dials, says hello as `machine_id`, and answers the challenge by
/// signing with `key`; the socket, the proof, and what the runner answered it.
async fn handshake(w: &World, machine_id: i64, key: &PrivateKey) -> (link::Dialed, Frame, Frame) {
    let mut ws = link::dial(&w.server).await.unwrap();
    let hello = Frame::Hello {
        machine_id,
        protocol: link::PROTOCOL,
        version: "test".to_owned(),
    };
    link::send(&mut ws, &hello).await.unwrap();
    let Frame::Challenge { nonce } = link::recv(&mut ws, Duration::from_secs(5)).await.unwrap()
    else {
        panic!("no challenge");
    };
    let proof = prove(key, &nonce, machine_id);
    link::send(&mut ws, &proof).await.unwrap();
    let answer = link::recv(&mut ws, Duration::from_secs(5)).await.unwrap();
    (ws, proof, answer)
}

fn prove(key: &PrivateKey, nonce: &str, machine_id: i64) -> Frame {
    let time = 1_700_000_000;
    let sig = key
        .sign(
            link::NAMESPACE,
            HashAlg::Sha512,
            &link::signed(nonce, machine_id, time),
        )
        .unwrap();
    Frame::Proof {
        time,
        signature: sig.to_pem(LineEnding::LF).unwrap(),
    }
}

fn is_refused(frame: &Frame) -> bool {
    matches!(frame, Frame::Refused { .. })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_linked_agent_is_online_offline_once_dropped_and_online_again_on_reconnect() {
    let w = world().await;
    let dir = agent_dir(&w, MACHINE, &w.key);
    let agent = spawn_agent(dir.path());
    reported(&w, true).await;
    agent.abort();
    reported(&w, false).await;
    let again = spawn_agent(dir.path());
    reported(&w, true).await;
    again.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_link_for_a_machine_replaces_the_first_and_stays_online() {
    let w = world().await;
    let (mut held, _, first) = handshake(&w, MACHINE, &w.key).await;
    assert!(matches!(first, Frame::Welcome { .. }), "{first:?}");
    reported(&w, true).await;
    let dir = agent_dir(&w, MACHINE, &w.key);
    let agent = spawn_agent(dir.path());
    let ended = loop {
        match link::recv(&mut held, Duration::from_secs(5)).await {
            Err(e) if e != "not a link frame" => break e,
            _ => {}
        }
    };
    assert_ne!(ended, "no answer in time", "the first link was not ended");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let now = w.links.lock().unwrap().clone().unwrap();
    assert_eq!(now[0]["online"], true, "{now}");
    agent.abort();
    reported(&w, false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proof_by_another_key_is_refused() {
    let w = world().await;
    let other = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    let (_, _, answer) = handshake(&w, MACHINE, &other).await;
    assert!(is_refused(&answer), "{answer:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proof_replayed_on_a_new_link_is_refused() {
    let w = world().await;
    let (_, proof, first) = handshake(&w, MACHINE, &w.key).await;
    assert!(matches!(first, Frame::Welcome { .. }), "{first:?}");
    let mut ws = link::dial(&w.server).await.unwrap();
    let hello = Frame::Hello {
        machine_id: MACHINE,
        protocol: link::PROTOCOL,
        version: "test".to_owned(),
    };
    link::send(&mut ws, &hello).await.unwrap();
    let challenge = link::recv(&mut ws, Duration::from_secs(5)).await.unwrap();
    assert!(
        matches!(challenge, Frame::Challenge { .. }),
        "{challenge:?}"
    );
    link::send(&mut ws, &proof).await.unwrap();
    let answer = link::recv(&mut ws, Duration::from_secs(5)).await.unwrap();
    assert!(is_refused(&answer), "{answer:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_that_is_not_enrolled_is_refused() {
    let w = world().await;
    let (_, _, answer) = handshake(&w, 99, &w.key).await;
    let Frame::Refused { reason } = answer else {
        panic!("{answer:?}");
    };
    assert!(reason.contains("not enrolled"), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_on_another_protocol_gets_update_required_and_nothing_else() {
    let w = world().await;
    let mut ws = link::dial(&w.server).await.unwrap();
    let hello = Frame::Hello {
        machine_id: MACHINE,
        protocol: link::PROTOCOL + 1,
        version: "test".to_owned(),
    };
    link::send(&mut ws, &hello).await.unwrap();
    let answer = link::recv(&mut ws, Duration::from_secs(5)).await.unwrap();
    assert!(matches!(answer, Frame::UpdateRequired { .. }), "{answer:?}");
    let after = link::recv(&mut ws, Duration::from_secs(5)).await;
    assert_eq!(after, Err("the link closed".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn run_needs_an_enrolled_state() {
    let w = world().await;
    let err = run(w.dir.path(), &|_| {}).await.unwrap_err();
    assert!(err.contains("enroll first"), "{err}");
}
