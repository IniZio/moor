//! The links broker: a local client reaches an agent over the runner's link.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use moor_client::link::boxes::{BoxAnswer, BoxOp};
use moor_client::link::broker::{self, SOCKET};
use moor_client::link::chan::{read_frame, Exit, Frame};
use moor_client::link::ops::{self, Op};
use moor_client::link::reach::Runner;
use moor_core::box_key::ControllerId;
use moor_machine_agent::{run_hosting, Machine, State};
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};

use crate::rig::{Rig, MACHINE_ID};

pub(crate) struct Host {
    pub(crate) rig: Rig,
    pub(crate) home: PathBuf,
    agent: Option<tokio::runtime::Runtime>,
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(agent) = self.agent.take() {
            agent.shutdown_background();
        }
    }
}

/// Writes machine `hv1`'s enrolled state and `key` into `state_dir`, for `server`.
fn enroll(state_dir: &Path, server: String, key: &PrivateKey) {
    let state = State {
        server,
        machine_id: MACHINE_ID,
        owner: "ada@example.com".to_owned(),
        name: "hv1".to_owned(),
        slots: 1,
        pool: None,
    };
    let state_json = serde_json::to_string(&state).unwrap();
    std::fs::write(state_dir.join("state.json"), state_json).unwrap();
    let text = key.to_openssh(LineEnding::LF).unwrap();
    std::fs::write(state_dir.join("id_ed25519"), text.as_bytes()).unwrap();
}

impl Host {
    /// As `boxes`, on a runner that also takes a controller's dials with [`KEY`].
    fn remote() -> Self {
        let args = ["--links", "127.0.0.1:0", "--service-key", KEY];
        Self::on(Rig::with_args(&args), true)
    }

    /// A machine that provides fake Boxes, linked to a runner.
    pub(crate) fn boxes() -> Self {
        Self::on(Rig::start(), true)
    }

    /// A machine that provides no Boxes, linked to a runner.
    fn tool() -> Self {
        Self::on(Rig::start(), false)
    }

    /// A machine linked to the runner of `rig`, providing fake Boxes when `boxes`.
    fn on(mut rig: Rig, boxes: bool) -> Self {
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
        let host = boxes.then(|| moor_machine_agent::Host::named("fake", &home).unwrap());
        let machine = Machine {
            home: home.clone(),
            shell: PathBuf::from("/bin/sh"),
            hooks: rig.dir().join("hooks"),
            allow_exec: false,
            sweep_grace: Duration::ZERO,
        };
        Self::linked(rig, home, (state_dir, machine, host))
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
    pub(crate) fn runner(&self) -> Runner {
        Runner::Local(self.rig.dir().join("runner.sock"))
    }

    fn socket(&self) -> PathBuf {
        self.rig.dir().join(SOCKET)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_with_no_link_is_named_offline() {
    let host = Host::boxes();
    let err = broker::status(&host.runner(), 99).await.unwrap_err();
    assert!(err.contains("machine 99 is offline"), "{err}");
}

#[test]
fn the_broker_socket_is_private_and_a_loose_one_stops_the_runner() {
    let host = Host::boxes();
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
    let status = broker::status(&runner, MACHINE_ID).await.unwrap();
    assert_eq!(status.os, std::env::consts::OS);
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
    let mut conn = box_stream(&host).await;
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
    let mut conn = box_stream(&host).await;
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

/// A Box stream to the rig's machine, retrying the moment the runner lags the rig's "online".
async fn box_stream(host: &Host) -> moor_client::link::reach::Conn {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match broker::boxes(&host.runner(), MACHINE_ID).await {
            Ok(conn) => return conn,
            Err(why) if why.contains("not linked") && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(why) => panic!("{why}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_provides_no_boxes_refuses_a_box_stream() {
    let host = Host::tool();
    let list = BoxOp::List {
        controller: ControllerId::parse("1a2b3c4d").unwrap(),
    };
    let mut conn = box_stream(&host).await;
    let said = box_answer(&mut conn, &list).await;
    let BoxAnswer::Refused { reason } = said else {
        panic!("{said:?}");
    };
    assert_eq!(reason, "this machine provides no Boxes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_box_stream_cannot_smuggle_any_other_op_past_the_agents_one_request() {
    let host = Host::boxes();
    let marker = host.home.join("ran");
    let mut conn = box_stream(&host).await;
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
