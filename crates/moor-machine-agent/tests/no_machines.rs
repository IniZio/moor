//! `run` against servers that cannot take a link.
#![allow(
    clippy::unwrap_used,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::Duration;

use moor_machine_agent::{run, Machine, State};
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};

fn agent_dir(server: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let state = State {
        server: server.to_owned(),
        machine_id: 7,
        owner: "ada@example.com".to_owned(),
        name: "minion".to_owned(),
        slots: 1,
    };
    std::fs::write(
        dir.path().join("state.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    let text = key.to_openssh(LineEnding::LF).unwrap();
    std::fs::write(dir.path().join("id_ed25519"), text.as_bytes()).unwrap();
    dir
}

/// A server that answers each connection with `reply`, or drops it when `reply` is empty.
fn server(reply: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut head = [0u8; 1024];
            let _ = stream.read(&mut head);
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    address
}

fn spawn_agent(dir: &Path) -> tokio::task::JoinHandle<Result<(), String>> {
    let dir = dir.to_owned();
    tokio::spawn(async move { run(&dir, Machine::from_env(), &|_| {}).await })
}

#[tokio::test]
async fn a_link_path_the_server_does_not_route_ends_run_naming_the_server() {
    let server = server("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let dir = agent_dir(&server);
    let ended = tokio::time::timeout(Duration::from_secs(10), spawn_agent(dir.path()))
        .await
        .expect("run kept dialing a server that takes no machines")
        .unwrap();
    let host = server.trim_start_matches("http://");
    assert_eq!(
        ended.unwrap_err(),
        format!(
            "the server at {host} takes no machines; its operator must enable the machine runner"
        )
    );
}

#[tokio::test]
async fn a_server_that_drops_the_link_is_dialed_again() {
    let server = server("");
    let dir = agent_dir(&server);
    let agent = spawn_agent(dir.path());
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!agent.is_finished(), "run gave up on a dropped link");
    agent.abort();
}
