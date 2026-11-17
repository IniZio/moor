//! The links broker: a local client reaches a pool host's leases over the runner's link.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use moor_client::link::boxes::{BoxAnswer, BoxOp};
use moor_client::link::broker::{self, SOCKET};
use moor_client::link::chan::{read_frame, Exit, Frame};
use moor_client::link::lease::{Answer, Boot, Verb};
use moor_client::link::ops::{self, Op};
use moor_client::link::reach::Runner;
use moor_core::box_key::ControllerId;
use moor_machine_agent::{run_hosting, Machine, Pool, State};
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
        Self::start_with(None, None, None)
    }

    /// As `start`, with the pool's `max`, `cache_gb`, and what `providers describe` prints.
    fn start_with(max: Option<usize>, cache_gb: Option<u32>, describe: Option<&str>) -> Self {
        Self::on(Rig::start(), max, cache_gb, describe)
    }

    /// As `start`, on a runner that also takes a controller's dials with [`KEY`].
    fn remote() -> Self {
        let args = ["--links", "127.0.0.1:0", "--service-key", KEY];
        Self::on(Rig::with_args(&args), None, None, None)
    }

    /// The pool host linked to the runner of `rig`.
    fn on(mut rig: Rig, max: Option<usize>, cache_gb: Option<u32>, describe: Option<&str>) -> Self {
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
        if let Some(describe) = describe {
            std::fs::write(home.join("describe.json"), describe).unwrap();
        }
        enroll(&state_dir, format!("http://127.0.0.1:{}", rig.agents), &key);
        let pool = Pool {
            provider: "fake".to_owned(),
            max,
            reserve_mib: None,
            cache_gb,
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
        Self::linked(rig, home, (state_dir, machine, None))
    }

    /// A machine that provides fake Boxes instead of a pool, linked to the runner of `rig`.
    fn boxes() -> Self {
        let mut rig = Rig::start();
        if let Some(agent) = rig.machine_agent.take() {
            agent.shutdown_background();
        }
        let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
        *rig.link.pubkey.lock().unwrap() = Some(key.public_key().to_openssh().unwrap());
        let state_dir = rig.dir().join("agent");
        let home = rig.dir().join("boxes-home");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        enroll(&state_dir, format!("http://127.0.0.1:{}", rig.agents), &key);
        let host = moor_machine_agent::Host::named("fake", &home).unwrap();
        let machine = Machine {
            home: home.clone(),
            shell: PathBuf::from("/bin/sh"),
            hooks: rig.dir().join("hooks"),
            pool: None,
            allow_exec: false,
            sweep_grace: Duration::ZERO,
        };
        Self::linked(rig, home, (state_dir, machine, Some(host)))
    }

    /// Runs the agent of `served` and waits for its link to come up.
    fn linked(
        rig: Rig,
        home: PathBuf,
        served: (PathBuf, Machine, Option<moor_machine_agent::Host>),
    ) -> Self {
        let (state_dir, machine, host) = served;
        let agent = tokio::runtime::Runtime::new().unwrap();
        agent.spawn(async move { run_hosting(&state_dir, machine, host, false, &|_| {}).await });
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

    /// The runner as the controller on its host reaches it.
    fn runner(&self) -> Runner {
        Runner::Local(self.rig.dir().join("runner.sock"))
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

#[test]
fn a_pool_hosts_offer_reaches_its_reported_link() {
    let describe = r#"{"capabilities":{"features":["snapshots","cache-volume"]}}"#;
    let host = Host::start_with(Some(3), Some(50), Some(describe));
    let links = host.rig.link.links.lock().unwrap().clone().unwrap();
    let link = &links[0];
    assert_eq!(link["max_boxes"], 3);
    assert_eq!(link["cache_gb"], 50);
    assert_eq!(
        link["features"],
        serde_json::json!(["cache-volume", "snapshots"])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_goes_through_the_broker_to_the_pool_host_and_back() {
    let host = Host::start();
    let runner = host.runner();
    let answer = broker::lease(
        &runner,
        MACHINE_ID,
        Verb::Warmup,
        Some("moor-1a2b3c4d-7"),
        Boot {
            cache: None,
            class: Some("fast".to_owned()),
        },
    )
    .await;
    assert_eq!(answer, Ok(Answer::Done));
    let calls = fake_crabbox::calls(&host.home);
    assert!(
        calls.ends_with(" warmup --provider fake --class fast --slug moor-1a2b3c4d-7\n"),
        "{calls}"
    );
    let answer = broker::lease(
        &runner,
        MACHINE_ID,
        Verb::Info,
        Some("moor-1a2b3c4d-7"),
        Boot::default(),
    )
    .await;
    let Ok(Answer::Info { user, host_keys }) = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(user, "vm");
    assert_eq!(host_keys, [fake_crabbox::HOST_KEY]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dial_through_the_broker_carries_bytes_both_ways() {
    let host = Host::start();
    let warmed = broker::lease(
        &host.runner(),
        MACHINE_ID,
        Verb::Warmup,
        Some("moor-1a2b3c4d-7"),
        Boot::default(),
    )
    .await;
    assert_eq!(warmed, Ok(Answer::Done));
    let mut vm = broker::dial(&host.runner(), MACHINE_ID, "moor-1a2b3c4d-7", None)
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
async fn a_port_through_the_broker_reaches_the_lease_tunnel_and_says_whether_it_listens() {
    let host = Host::start();
    let slug = "moor-1a2b3c4d-7";
    let runner = host.runner();
    let warmed = broker::lease(
        &runner,
        MACHINE_ID,
        Verb::Warmup,
        Some(slug),
        Boot::default(),
    );
    assert_eq!(warmed.await, Ok(Answer::Done));
    // The fake's tunnel listens where this echo server does.
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        host.home.join("tunnel.port"),
        echo.local_addr().unwrap().port().to_string(),
    )
    .unwrap();
    tokio::spawn(async move {
        while let Ok((mut conn, _)) = echo.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 64];
                while let Ok(n @ 1..) = conn.read(&mut buf).await {
                    let _ = conn.write_all(&buf[..n]).await;
                }
            });
        }
    });
    let mut app = broker::port(&runner, MACHINE_ID, slug, 5177).await.unwrap();
    app.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
    let mut back = [0u8; 16];
    tokio::time::timeout(Duration::from_secs(10), app.read_exact(&mut back))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&back, b"GET / HTTP/1.1\r\n");
    assert_eq!(
        broker::listens(&runner, MACHINE_ID, slug, 5177).await,
        Ok(true)
    );
    let refused = broker::port(&runner, MACHINE_ID, "not-a-slug", 5177).await;
    assert!(refused.is_err_and(|e| e.contains("is not a pool lease slug")));
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
    let warmed = broker::lease(
        &host.runner(),
        MACHINE_ID,
        Verb::Warmup,
        Some("moor-1a2b3c4d-7"),
        Boot::default(),
    )
    .await;
    assert_eq!(warmed, Ok(Answer::Done));
    let bad = broker::lease(
        &host.runner(),
        MACHINE_ID,
        Verb::Info,
        Some("box-1"),
        Boot::default(),
    )
    .await;
    assert!(bad.is_err());
    let leases = r#"[{"name":"moor-1a2b3c4d-7","state":"running"}]"#;
    std::fs::write(host.home.join("leases.json"), leases).unwrap();
    let status = broker::status(&host.runner(), MACHINE_ID).await.unwrap();
    assert!(!status.version.is_empty());
    let pool = status.pool.unwrap();
    assert_eq!(pool.provider, "fake");
    assert_eq!(pool.leases.len(), 1, "{pool:?}");
    assert_eq!(pool.leases[0].slug, "moor-1a2b3c4d-7");
    let ops: Vec<_> = status
        .recent
        .iter()
        .map(|s| (s.op.as_str(), s.target.as_deref(), s.error.is_some()))
        .collect();
    assert_eq!(
        ops,
        [
            ("lease", Some("moor-1a2b3c4d-7"), false),
            ("lease", Some("box-1"), true)
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_with_no_link_is_named_offline() {
    let host = Host::start();
    let answer = raw(
        &host.socket(),
        "{\"machine_id\":99,\"op\":{\"op\":\"dial\",\"slug\":\"moor-1a2b3c4d-7\"}}\n",
    )
    .await;
    assert_eq!(answer["broker"], "offline", "{answer}");
    assert_eq!(answer["machine_id"], 99);
    let err = broker::lease(&host.runner(), 99, Verb::List, None, Boot::default())
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

/// The machine service key the [`Host::remote`] runner checks.
const KEY: &str = "machine-service-key";

impl Host {
    /// The runner as a controller in another pod reaches it, presenting `key`.
    fn remote_runner(&self, key: &str) -> Runner {
        Runner::Remote {
            addr: format!("127.0.0.1:{}", self.rig.links.unwrap()),
            key: key.to_owned(),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_controller_reaches_the_machine_through_the_runner_across_the_network_that_holds_it() {
    let host = Host::remote();
    let mut idle = Rig::with_args(&["--links", "127.0.0.1:0", "--service-key", KEY]);
    if let Some(agent) = idle.machine_agent.take() {
        agent.shutdown_background();
    }
    idle.online(false);
    let elsewhere = Runner::Remote {
        addr: format!("127.0.0.1:{}", idle.links.unwrap()),
        key: KEY.to_owned(),
    };
    let err = broker::status(&elsewhere, MACHINE_ID).await.unwrap_err();
    assert!(err.contains("is offline"), "{err}");

    let runner = host.remote_runner(KEY);
    let warmed = broker::lease(
        &runner,
        MACHINE_ID,
        Verb::Warmup,
        Some("moor-1a2b3c4d-7"),
        Boot::default(),
    )
    .await;
    assert_eq!(warmed, Ok(Answer::Done));
    let status = broker::status(&runner, MACHINE_ID).await.unwrap();
    assert!(status.pool.is_some(), "{status:?}");
    let mut vm = broker::dial(&runner, MACHINE_ID, "moor-1a2b3c4d-7", None)
        .await
        .unwrap();
    vm.write_all(b"SSH-2.0-hello\n").await.unwrap();
    let mut back = [0u8; 14];
    tokio::time::timeout(Duration::from_secs(10), vm.read_exact(&mut back))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&back, b"SSH-2.0-hello\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_controller_with_the_wrong_key_is_refused_links_and_forwards() {
    use moor_client::forward::{Forward, Forwarding};
    use moor_client::link::ops;
    use moor_client::link::reach::Service;

    let host = Host::remote();
    let wrong = host.remote_runner("not-the-key");
    let err = broker::status(&wrong, MACHINE_ID).await.unwrap_err();
    assert!(err.contains("the machine service key is wrong"), "{err}");

    let mut conn = wrong.connect(Service::Forwards).await.unwrap();
    let ask = Forward {
        box_id: 7,
        machine_id: MACHINE_ID,
        ports: vec![8080],
    };
    ops::write_line(&mut conn, &ask).await.unwrap();
    let answer: Forwarding = ops::read_line(&mut conn).await.unwrap();
    assert_eq!(
        answer,
        Forwarding::Refused {
            reason: "the machine service key is wrong".to_owned()
        }
    );
    assert_eq!(fake_crabbox::calls(&host.home), "");
}

/// Writes `op` on a Box stream and reads the answer line.
async fn box_answer(conn: &mut moor_client::link::reach::Conn, op: &BoxOp) -> BoxAnswer {
    ops::write_line(conn, op).await.unwrap();
    ops::read_line(conn).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_box_stream_is_spliced_to_an_agent_that_serves_it_unread() {
    let host = Host::boxes();
    let slug = "moor-1a2b3c4d-417".to_owned();
    let open = BoxOp::Open {
        slug: slug.clone(),
        class: None,
        cache: None,
    };
    let mut conn = broker::boxes(&host.runner(), MACHINE_ID).await.unwrap();
    let mut said = box_answer(&mut conn, &open).await;
    if matches!(said, BoxAnswer::Note { .. }) {
        said = ops::read_line(&mut conn).await.unwrap();
    }
    assert!(
        matches!(said, BoxAnswer::Opened { generation: 1, .. }),
        "{said:?}"
    );
    let exec = BoxOp::Exec {
        slug,
        place: moor_core::seam::driver::Place::Work,
        argv: ["sh", "-c", "echo $MOOR_PLACE; exit 4"]
            .map(str::to_owned)
            .to_vec(),
        env: Vec::new(),
        cwd: None,
        stdin: false,
        tty: None,
    };
    let mut conn = broker::boxes(&host.runner(), MACHINE_ID).await.unwrap();
    assert_eq!(box_answer(&mut conn, &exec).await, BoxAnswer::Streaming);
    let mut buf = Vec::new();
    let mut out = String::new();
    loop {
        match read_frame(&mut conn, &mut buf).await.unwrap() {
            Some(Frame::Out(bytes)) => out.push_str(&String::from_utf8_lossy(bytes)),
            Some(Frame::Exit(exit)) => {
                assert_eq!(exit, Exit::Code(4));
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(out, "work\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_provides_no_boxes_refuses_a_box_stream() {
    let host = Host::start();
    let list = BoxOp::List {
        controller: ControllerId::parse("1a2b3c4d").unwrap(),
    };
    let mut conn = broker::boxes(&host.runner(), MACHINE_ID).await.unwrap();
    let said = box_answer(&mut conn, &list).await;
    let BoxAnswer::Refused { reason } = said else {
        panic!("{said:?}");
    };
    assert_eq!(reason, "this machine provides no Boxes");
    assert_eq!(fake_crabbox::calls(&host.home), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_box_stream_cannot_smuggle_any_other_op_past_the_agents_one_request() {
    let host = Host::boxes();
    let marker = host.home.join("ran");
    let mut conn = broker::boxes(&host.runner(), MACHINE_ID).await.unwrap();
    let exec = Op::Exec {
        box_id: 7,
        env: Vec::new(),
        script: format!("touch {}", marker.display()),
        timeout_secs: 5,
    };
    ops::write_line(&mut conn, &exec).await.unwrap();
    let said: BoxAnswer = ops::read_line(&mut conn).await.unwrap();
    assert!(matches!(said, BoxAnswer::Refused { .. }), "{said:?}");
    assert!(!marker.exists());
}
