//! A host that stops the agent with SIGTERM or SIGINT gets a clean exit, so a service manager
//! does not count the stop as a failure.
#![allow(clippy::unwrap_used, reason = "tests")]

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Command, Stdio};

use moor_machine_agent::State;
use moor_testkit::ChildGuard;
use rustix::process::{kill_process, Pid, Signal};
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};

/// Runs the agent against a server that drops every link, stops it with `signal` once it
/// has logged a failed dial, and returns its exit code and stderr.
fn stopped_with(signal: Signal) -> (Option<i32>, String) {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", server.local_addr().unwrap());
    std::thread::spawn(move || for _ in server.incoming().flatten() {});
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("moor-machine-agent");
    std::fs::create_dir_all(&dir).unwrap();
    let state = State {
        server: address,
        machine_id: 7,
        owner: "ada@example.com".to_owned(),
        name: "minion".to_owned(),
        slots: 1,
        pool: None,
    };
    std::fs::write(
        dir.join("state.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    std::fs::write(
        dir.join("id_ed25519"),
        key.to_openssh(LineEnding::LF).unwrap().as_bytes(),
    )
    .unwrap();

    let mut agent = ChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_moor-machine-agent"))
            .arg("run")
            .env("HOME", home.path())
            .env("XDG_STATE_HOME", home.path())
            .env_remove("XDG_CONFIG_HOME")
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut lines = BufReader::new(agent.stderr.take().unwrap()).lines();
    let mut seen = String::new();
    // A dial that failed proves the signal handlers are already installed.
    for line in lines.by_ref() {
        let line = line.unwrap();
        seen.push_str(&line);
        seen.push('\n');
        if line.contains("dialing again") {
            break;
        }
    }
    kill_process(Pid::from_child(&agent), signal).unwrap();
    for line in lines {
        seen.push_str(&line.unwrap());
        seen.push('\n');
    }
    (agent.wait().unwrap().code(), seen)
}

#[test]
fn sigterm_after_a_clean_shutdown_exits_zero() {
    let (code, err) = stopped_with(Signal::TERM);
    assert_eq!(code, Some(0), "{err}");
    assert!(err.contains("stopped by SIGTERM"), "{err}");
}

#[test]
fn sigint_after_a_clean_shutdown_exits_zero() {
    let (code, err) = stopped_with(Signal::INT);
    assert_eq!(code, Some(0), "{err}");
    assert!(err.contains("stopped by SIGINT"), "{err}");
}
