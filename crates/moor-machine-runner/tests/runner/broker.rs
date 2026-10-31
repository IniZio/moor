//! The links broker: a local client reaches a pool host's leases over the runner's link.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use moor_client::link::broker::{self, SOCKET};
use moor_client::link::lease::{Answer, Verb};
use moor_machine_agent::{run, Machine, Pool, State};
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::rig::{Rig, MACHINE_ID};

#[path = "../../../moor-machine-agent/tests/fake_crabbox/mod.rs"]
mod fake_crabbox;

struct Host {
    rig: Rig,
    home: PathBuf,
    agent: Option<tokio::runtime::Runtime>,
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(agent) = self.agent.take() {
            agent.shutdown_background();
        }
    }
}

/// Writes pool host `hv1`'s enrolled state and `key` into `state_dir`, for `server`.
fn enroll(state_dir: &Path, server: String, key: &PrivateKey) {
    let state = State {
        server,
        machine_id: MACHINE_ID,
        owner: "ada@example.com".to_owned(),
        name: "hv1".to_owned(),
        slots: 1,
    };
    let state_json = serde_json::to_string(&state).unwrap();
    std::fs::write(state_dir.join("state.json"), state_json).unwrap();
    let text = key.to_openssh(LineEnding::LF).unwrap();
    std::fs::write(state_dir.join("id_ed25519"), text.as_bytes()).unwrap();
}

impl Host {
    /// A runner taking links, and a pool host serving provider `fake` linked to it.
    fn start() -> Self {
        let mut rig = Rig::start();
        // The rig links an ordinary machine under the same id; the pool host replaces it.
        if let Some(agent) = rig.machine_agent.take() {
            agent.shutdown_background();
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        while rig.link.links.lock().unwrap().as_ref().is_some_and(|l| {
            l.as_array()
                .is_some_and(|l| l.iter().any(|m| m["online"] == true))
        }) {
            assert!(Instant::now() < deadline, "the rig's agent never unlinked");
            std::thread::sleep(Duration::from_millis(50));
        }
        let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
        *rig.link.pubkey.lock().unwrap() = Some(key.public_key().to_openssh().unwrap());
        let state_dir = rig.dir().join("agent");
        let home = rig.dir().join("pool-home");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        enroll(&state_dir, format!("http://127.0.0.1:{}", rig.agents), &key);
        let pool = Pool {
            provider: "fake".to_owned(),
            max: None,
            crabbox: fake_crabbox::install(&home),
        };
        let machine = Machine {
            home: home.clone(),
            shell: PathBuf::from("/bin/sh"),
            hooks: rig.dir().join("hooks"),
            pool: Some(pool),
            allow_exec: false,
            sweep_grace: Duration::ZERO,
        };
        let agent = tokio::runtime::Runtime::new().unwrap();
        agent.spawn(async move { run(&state_dir, machine, &|_| {}).await });
        let deadline = Instant::now() + Duration::from_secs(15);
        while !rig
            .link
            .links
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|l| l[0]["machine_id"] == MACHINE_ID && l[0]["online"] == true)
        {
            assert!(Instant::now() < deadline, "the agent never linked");
            std::thread::sleep(Duration::from_millis(50));
        }
        Self {
            rig,
            home,
            agent: Some(agent),
        }
    }

    fn socket(&self) -> PathBuf {
        self.rig.dir().join(SOCKET)
    }
}

/// Sends `line` to the broker at `socket` and reads its one-line answer.
async fn raw(socket: &Path, line: &str) -> serde_json::Value {
    let mut conn = BufReader::new(UnixStream::connect(socket).await.unwrap());
    conn.get_mut().write_all(line.as_bytes()).await.unwrap();
    let mut answer = String::new();
    conn.read_line(&mut answer).await.unwrap();
    serde_json::from_str(&answer).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_goes_through_the_broker_to_the_pool_host_and_back() {
    let host = Host::start();
    let socket = host.socket();
    let answer = broker::lease(&socket, MACHINE_ID, Verb::Warmup, Some("moor-7")).await;
    assert_eq!(answer, Ok(Answer::Done));
    let calls = fake_crabbox::calls(&host.home);
    assert!(
        calls.ends_with(" warmup --provider fake --slug moor-7\n"),
        "{calls}"
    );
    let answer = broker::lease(&socket, MACHINE_ID, Verb::Info, Some("moor-7")).await;
    let Ok(Answer::Info { user, host_keys }) = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(user, "vm");
    assert_eq!(host_keys, [fake_crabbox::HOST_KEY]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dial_through_the_broker_carries_bytes_both_ways() {
    let host = Host::start();
    let warmed = broker::lease(&host.socket(), MACHINE_ID, Verb::Warmup, Some("moor-7")).await;
    assert_eq!(warmed, Ok(Answer::Done));
    let mut vm = broker::dial(&host.socket(), MACHINE_ID, "moor-7")
        .await
        .unwrap();
    for hello in [&b"SSH-2.0-hello\n"[..], b"second\n"] {
        vm.write_all(hello).await.unwrap();
        let mut back = vec![0u8; hello.len()];
        tokio::time::timeout(Duration::from_secs(10), vm.read_exact(&mut back))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(back, hello);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_broker_refuses_every_op_but_lease_dial_and_status() {
    let host = Host::start();
    let marker = host.home.join("ran");
    let exec = format!(
        r#"{{"op":"exec","box_id":7,"env":[],"script":"touch {}","timeout_secs":30}}"#,
        marker.display()
    );
    for op in [
        exec.as_str(),
        r#"{"op":"listen","box_id":7,"listen":1,"ports":[8080]}"#,
        r#"{"op":"lease","verb":"list","provider":"evil"}"#,
    ] {
        let line = format!("{{\"machine_id\":{MACHINE_ID},\"op\":{op}}}\n");
        let answer = raw(&host.socket(), &line).await;
        assert_eq!(answer["broker"], "refused", "{op}: {answer}");
    }
    assert!(!marker.exists());
    assert_eq!(fake_crabbox::calls(&host.home), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pool_host_reports_its_status_through_the_broker() {
    let host = Host::start();
    let warmed = broker::lease(&host.socket(), MACHINE_ID, Verb::Warmup, Some("moor-7")).await;
    assert_eq!(warmed, Ok(Answer::Done));
    let bad = broker::lease(&host.socket(), MACHINE_ID, Verb::Info, Some("box-1")).await;
    assert!(bad.is_err());
    let leases = r#"[{"name":"moor-7","state":"running"}]"#;
    std::fs::write(host.home.join("leases.json"), leases).unwrap();
    let status = broker::status(&host.socket(), MACHINE_ID).await.unwrap();
    assert_eq!(status.protocol, moor_client::link::PROTOCOL);
    assert!(!status.version.is_empty());
    let pool = status.pool.unwrap();
    assert_eq!(pool.provider, "fake");
    assert_eq!(pool.leases.len(), 1, "{pool:?}");
    assert_eq!(pool.leases[0].slug, "moor-7");
    let ops: Vec<_> = status
        .recent
        .iter()
        .map(|s| (s.op.as_str(), s.target.as_deref(), s.error.is_some()))
        .collect();
    assert_eq!(
        ops,
        [
            ("lease", Some("moor-7"), false),
            ("lease", Some("box-1"), true)
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_with_no_link_is_named_offline() {
    let host = Host::start();
    let answer = raw(
        &host.socket(),
        "{\"machine_id\":99,\"op\":{\"op\":\"dial\",\"slug\":\"moor-7\"}}\n",
    )
    .await;
    assert_eq!(answer["broker"], "offline", "{answer}");
    assert_eq!(answer["machine_id"], 99);
    let err = broker::lease(&host.socket(), 99, Verb::List, None)
        .await
        .unwrap_err();
    assert!(err.contains("machine 99 is offline"), "{err}");
}

#[test]
fn the_broker_socket_is_private_and_a_loose_one_stops_the_runner() {
    let host = Host::start();
    let mode = std::fs::metadata(host.socket())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    std::fs::set_permissions(host.socket(), std::fs::Permissions::from_mode(0o666)).unwrap();
    let dir = host.rig.dir();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_moor-machine-runner"))
        .args([
            "--listen",
            "127.0.0.1:0",
            "--agents",
            "127.0.0.1:0",
            "--socket",
        ])
        .arg(dir.join("runner.sock"))
        .arg("--state-dir")
        .arg(dir.join("state"))
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("open to other users"), "{err}");
    let mode = std::fs::metadata(host.socket())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o777,
        0o666,
        "the loose socket is left for its owner to look at"
    );
}
