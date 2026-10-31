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

use moor_client::link::ops::{self, Reply};
use moor_client::link::{self, Frame};
use moor_core::response::Response;
use moor_machine_agent::{run, State};
use serde_json::{json, Value};
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey};
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod fake_crabbox;

const MACHINE: i64 = 7;

struct World {
    server: String,
    key: PrivateKey,
    links: Arc<Mutex<Option<Value>>>,
    dir: tempfile::TempDir,
    agents: moor_machine_runner::Agents,
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
    let agents = moor_machine_runner::Agents::new(&socket);
    let serving = agents.clone();
    tokio::spawn(async move { moor_machine_runner::serve_agents(listener, serving).await });
    World {
        server,
        key,
        links,
        dir,
        agents,
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

fn machine() -> moor_machine_agent::Machine {
    moor_machine_agent::Machine::from_env()
}

fn spawn_agent(dir: &Path) -> tokio::task::JoinHandle<Result<(), String>> {
    let dir = dir.to_owned();
    tokio::spawn(async move { run(&dir, machine(), &|_| {}).await })
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

/// A hello from `machine_id` speaking `protocol`.
fn hello(machine_id: i64, protocol: u32) -> Frame {
    Frame::Hello {
        machine_id,
        protocol,
        version: "test".to_owned(),
        offer: None,
    }
}

/// Dials, says hello as `machine_id`, and answers the challenge by
/// signing with `key`; the socket, the proof, and what the runner answered it.
async fn handshake(w: &World, machine_id: i64, key: &PrivateKey) -> (link::Dialed, Frame, Frame) {
    let mut ws = link::dial(&w.server).await.unwrap();
    let hello = hello(machine_id, link::PROTOCOL);
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
    let hello = hello(MACHINE, link::PROTOCOL);
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
    let hello = hello(MACHINE, link::PROTOCOL + 1);
    link::send(&mut ws, &hello).await.unwrap();
    let answer = link::recv(&mut ws, Duration::from_secs(5)).await.unwrap();
    assert!(matches!(answer, Frame::UpdateRequired { .. }), "{answer:?}");
    let after = link::recv(&mut ws, Duration::from_secs(5)).await;
    assert_eq!(after, Err("the link closed".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn run_needs_an_enrolled_state() {
    let w = world().await;
    let err = run(w.dir.path(), machine(), &|_| {}).await.unwrap_err();
    assert!(err.contains("enroll first"), "{err}");
}

/// An agent for machine 7 linked to `w`'s runner, its home and hooks under `home`.
async fn linked(w: &World, home: &Path) -> tempfile::TempDir {
    linked_as(w, home, None, true).await
}

/// Links an agent serving `pool`, or no pool, from `home`.
async fn linked_as(
    w: &World,
    home: &Path,
    pool: Option<moor_machine_agent::Pool>,
    allow_exec: bool,
) -> tempfile::TempDir {
    let machine = moor_machine_agent::Machine {
        home: home.to_owned(),
        shell: "/bin/sh".into(),
        hooks: home.join("hooks"),
        pool,
        allow_exec,
        sweep_grace: Duration::ZERO,
    };
    linked_machine(w, machine).await
}

/// Links an agent serving `machine`.
async fn linked_machine(w: &World, machine: moor_machine_agent::Machine) -> tempfile::TempDir {
    let dir = agent_dir(w, MACHINE, &w.key);
    let state = dir.path().to_owned();
    tokio::spawn(async move { run(&state, machine, &|_| {}).await });
    reported(w, true).await;
    dir
}

/// Opens a stream to machine 7 and sends `op`; the stream and the agent's reply.
async fn ask(w: &World, op: &Value) -> (link::Channel, Reply) {
    let mut ch = w.agents.open(MACHINE).await.unwrap();
    ops::write_line(&mut ch, op).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(30), ops::read_line(&mut ch))
        .await
        .unwrap()
        .unwrap();
    (ch, reply)
}

fn ran(reply: Reply) -> ops::Ran {
    match reply {
        Reply::Ran(ran) => ran,
        other => panic!("not ran: {other:?}"),
    }
}

fn hook_file(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn exec_runs_in_the_box_dir_with_its_env() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = linked(&w, home.path()).await;
    let op = json!({"op": "exec", "box_id": 7, "env": [["MOOR_FACT_DEVICE", "sim 1"]],
        "script": "pwd; echo \"$MOOR_FACT_DEVICE\"; echo \"$TMPDIR\"; exit 3", "timeout_secs": 30});
    let out = ran(ask(&w, &op).await.1);
    let root = home.path().canonicalize().unwrap();
    let want = format!("{0}/moor/7\nsim 1\n{0}/moor-state/7/tmp\n", root.display());
    assert_eq!(
        (out.exit_code, out.output.as_str()),
        (Some(3), want.as_str())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn exec_is_refused_unless_the_agent_was_started_with_allow_exec() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = linked_as(&w, home.path(), None, false).await;
    let marker = home.path().join("ran");
    let op = json!({"op": "exec", "box_id": 7, "env": [], "timeout_secs": 30,
        "script": format!("touch {}", marker.display())});
    let Reply::Refused { reason } = ask(&w, &op).await.1 else {
        panic!("exec was served");
    };
    assert!(reason.contains("--allow-exec"), "{reason}");
    assert!(!marker.exists());
    let hook = json!({"op": "hook", "box_id": 7, "hook": "nope", "env": [], "timeout_secs": 30});
    let reply = ask(&w, &hook).await.1;
    assert!(
        !matches!(&reply, Reply::Refused { reason } if reason.contains("--allow-exec")),
        "{reply:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn exec_keeps_the_last_64_kib_of_output() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = linked(&w, home.path()).await;
    let op = json!({"op": "exec", "box_id": 7, "env": [],
        "script": "head -c 200000 /dev/zero | tr '\\0' a; echo end", "timeout_secs": 30});
    let out = ran(ask(&w, &op).await.1);
    assert!(out.truncated);
    assert_eq!(out.output.len(), 64 * 1024);
    assert!(out.output.ends_with("aaaend\n"));
}

#[tokio::test(flavor = "multi_thread")]
async fn exec_past_its_timeout_is_killed() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = linked(&w, home.path()).await;
    let op = json!({"op": "exec", "box_id": 7, "env": [], "script": "sleep 60", "timeout_secs": 1});
    let out = ran(ask(&w, &op).await.1);
    assert_eq!((out.exit_code, out.timed_out), (None, true));
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_stream_ends_the_commands_process_group() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = linked(&w, home.path()).await;
    let pid_file = home.path().join("sleeper");
    let script = format!("sleep 300 & echo $! > {}; wait", pid_file.display());
    let mut ch = w.agents.open(MACHINE).await.unwrap();
    let op = json!({"op": "exec", "box_id": 7, "env": [], "script": script, "timeout_secs": 600});
    ops::write_line(&mut ch, &op).await.unwrap();
    let pid = poll(|| {
        std::fs::read_to_string(&pid_file)
            .ok()
            .filter(|p| p.ends_with('\n'))
    })
    .await;
    let alive = |pid: &str| {
        std::process::Command::new("ps")
            .args(["-p", pid.trim()])
            .output()
            .unwrap()
            .status
            .success()
    };
    assert!(alive(&pid), "the sleeper runs while the stream is open");
    drop(ch);
    poll(|| (!alive(&pid)).then_some(())).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hook_runs_by_name_and_prints_its_facts_last() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = linked(&w, home.path()).await;
    hook_file(
        &home.path().join("hooks/prepare"),
        "#!/bin/sh\necho '{\"device\":\"d1\"}'\necho noise >&2\n",
    );
    let op = json!({"op": "hook", "box_id": 7, "hook": "prepare", "env": [], "timeout_secs": 30});
    let out = ran(ask(&w, &op).await.1);
    assert_eq!(
        (out.exit_code, out.output.as_str()),
        (Some(0), "noise\n{\"device\":\"d1\"}\n")
    );
    let op = json!({"op": "hook", "box_id": 7, "hook": "release", "env": [], "timeout_secs": 30});
    assert_eq!(ask(&w, &op).await.1, Reply::Missing);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_hook_or_a_sent_command_is_refused() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = linked(&w, home.path()).await;
    hook_file(&home.path().join("hooks/prepare"), "#!/bin/sh\necho '{}'\n");
    hook_file(
        &home.path().join("hooks/evil"),
        "#!/bin/sh\ntouch \"$HOME/pwned\"\n",
    );
    let pwned = format!("touch {}/pwned", home.path().display());
    for op in [
        json!({"op": "hook", "box_id": 7, "hook": "evil", "env": [], "timeout_secs": 30}),
        json!({"op": "hook", "box_id": 7, "hook": "prepare", "command": pwned, "env": [],
            "timeout_secs": 30}),
        json!({"op": "relay", "box_id": 7, "name": "../evil", "env": []}),
    ] {
        let reply = ask(&w, &op).await.1;
        assert!(matches!(reply, Reply::Refused { .. }), "{op}: {reply:?}");
    }
    assert!(!home.path().join("pwned").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_carries_its_servers_stdio() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = linked(&w, home.path()).await;
    hook_file(
        &home.path().join("hooks/relays/echo"),
        "#!/bin/sh\necho \"$MOOR_BOX\"\nexec cat\n",
    );
    let op = json!({"op": "relay", "box_id": 7, "name": "echo", "env": [["MOOR_BOX", "7"]]});
    let (mut ch, reply) = ask(&w, &op).await;
    assert_eq!(reply, Reply::Started);
    ch.write_all(b"ping\n").await.unwrap();
    let mut got = vec![0u8; 7];
    tokio::time::timeout(Duration::from_secs(10), ch.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, b"7\nping\n");
    let op = json!({"op": "relay", "box_id": 7, "name": "none", "env": []});
    assert_eq!(ask(&w, &op).await.1, Reply::Missing);
}

/// Waits up to 10 s for `f` to give something.
async fn poll<T>(mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Links a pool host serving provider `fake` with the fake crabbox in `home`.
async fn pool_host(w: &World, home: &Path) -> tempfile::TempDir {
    let pool = moor_machine_agent::Pool {
        provider: "fake".to_owned(),
        max: None,
        cache_gb: None,
        crabbox: fake_crabbox::install(home),
    };
    linked_as(w, home, Some(pool), false).await
}

fn refused(reply: &Reply) -> bool {
    matches!(reply, Reply::Refused { .. })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pool_host_refuses_every_op_but_lease_and_dial() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = pool_host(&w, home.path()).await;
    let marker = home.path().join("ran");
    let ops = [
        json!({"op": "exec", "box_id": 7, "env": [], "script": format!("touch {}", marker.display()),
            "timeout_secs": 30}),
        json!({"op": "hook", "box_id": 7, "hook": "prepare", "env": [], "timeout_secs": 30}),
        json!({"op": "relay", "box_id": 7, "name": "r", "env": []}),
        json!({"op": "sync", "box_id": 7, "argv": ["rsync", "--server", "."]}),
        json!({"op": "pull", "box_id": 7, "path": "x"}),
        json!({"op": "listen", "box_id": 7, "listen": 1, "ports": [8080]}),
        json!({"op": "remove", "box_id": 3}),
    ];
    std::fs::create_dir_all(home.path().join("moor/3")).unwrap();
    for op in &ops {
        let reply = ask(&w, op).await.1;
        assert!(refused(&reply), "{op}: {reply:?}");
    }
    assert!(!marker.exists());
    let swept = swept(ask(&w, &json!({"op": "sweep", "keep": []})).await.1);
    assert_eq!((swept.removed, swept.errors), (vec![], vec![]));
    assert!(
        home.path().join("moor/3").is_dir(),
        "a pool host sweeps nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_that_is_no_pool_host_refuses_lease_and_dial() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = linked(&w, home.path()).await;
    fake_crabbox::install(home.path());
    for op in [
        json!({"op": "lease", "verb": "list"}),
        json!({"op": "dial", "slug": "moor-1a2b3c4d-7"}),
    ] {
        let reply = ask(&w, &op).await.1;
        assert!(refused(&reply), "{op}: {reply:?}");
    }
    assert_eq!(fake_crabbox::calls(home.path()), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_uses_the_hosts_provider_and_a_request_cannot_name_one() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = pool_host(&w, home.path()).await;
    let named =
        json!({"op": "lease", "verb": "warmup", "slug": "moor-1a2b3c4d-7", "provider": "evil"});
    assert!(refused(&ask(&w, &named).await.1));
    assert_eq!(fake_crabbox::calls(home.path()), "");
    let op = json!({"op": "lease", "verb": "warmup", "slug": "moor-1a2b3c4d-7"});
    let reply = ask(&w, &op).await.1;
    assert_eq!(reply, Reply::Leased(moor_client::link::lease::Answer::Done));
    assert!(fake_crabbox::calls(home.path())
        .ends_with(" warmup --provider fake --slug moor-1a2b3c4d-7\n"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dial_carries_the_stream_to_the_vms_sshd_and_back() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = pool_host(&w, home.path()).await;
    let warmup = json!({"op": "lease", "verb": "warmup", "slug": "moor-1a2b3c4d-7"});
    assert!(!refused(&ask(&w, &warmup).await.1));
    let (mut ch, reply) = ask(&w, &json!({"op": "dial", "slug": "moor-1a2b3c4d-7"})).await;
    assert_eq!(reply, Reply::Started);
    ch.write_all(b"SSH-2.0-hello\n").await.unwrap();
    let mut back = [0u8; 14];
    tokio::time::timeout(Duration::from_secs(10), ch.read_exact(&mut back))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&back, b"SSH-2.0-hello\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_on_a_lease_never_warmed_leaves_no_dir_on_the_host() {
    let (w, home) = (world().await, tempfile::tempdir().unwrap());
    let _agent = pool_host(&w, home.path()).await;
    let info = json!({"op": "lease", "verb": "info", "slug": "moor-1a2b3c4d-7"});
    ask(&w, &info).await;
    let stop = json!({"op": "lease", "verb": "stop", "slug": "moor-1a2b3c4d-7"});
    assert_eq!(
        ask(&w, &stop).await.1,
        Reply::Leased(moor_client::link::lease::Answer::Done)
    );
    assert!(!home.path().join("moor-pool/moor-1a2b3c4d-7").exists());
}

#[path = "link/sweep.rs"]
mod sweep;
use sweep::swept;
