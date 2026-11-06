//! A runner that is stopping tells its agents to relink; a runner admits a hello while the
//! controller is briefly away.

use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use super::*;

/// Takes one agent's link on `listener` as a runner that checks no key.
async fn take(listener: &TcpListener) -> link::Carried {
    let (tcp, _) = listener.accept().await.unwrap();
    let mut ws = link::accept(tcp).await.unwrap();
    let hello = link::recv(&mut ws, Duration::from_secs(5)).await.unwrap();
    assert!(matches!(hello, Frame::Hello { .. }), "{hello:?}");
    let challenge = Frame::Challenge {
        nonce: "n".to_owned(),
    };
    link::send(&mut ws, &challenge).await.unwrap();
    link::recv(&mut ws, Duration::from_secs(5)).await.unwrap();
    let welcome = Frame::Welcome {
        version: "test".to_owned(),
    };
    link::send(&mut ws, &welcome).await.unwrap();
    link::carry(ws, link::KEEPALIVE)
}

#[tokio::test(flavor = "multi_thread")]
async fn relink_dials_again_at_once_and_the_old_links_exec_still_answers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    let home = tempfile::tempdir().unwrap();
    let w = World {
        server: format!("http://{}", listener.local_addr().unwrap()),
        key: key.clone(),
        links: Arc::default(),
        dir: tempfile::tempdir().unwrap(),
        agents: moor_machine_runner::Agents::new(&home.path().join("none.sock")),
    };
    let dir = agent_dir(&w, MACHINE, &key);
    let machine = moor_machine_agent::Machine {
        home: home.path().to_owned(),
        shell: "/bin/sh".into(),
        hooks: home.path().join("hooks"),
        pool: None,
        allow_exec: true,
        sweep_grace: Duration::ZERO,
    };
    let state = dir.path().to_owned();
    let agent = tokio::spawn(async move { run(&state, machine, &|_| {}).await });

    let old = take(&listener).await;
    let (ask, asks) = mpsc::channel(1);
    let old_link = tokio::spawn(link::drive(
        old.socket,
        link::Mode::Server,
        asks,
        |_| {},
        std::future::pending(),
    ));
    let (opened, ch) = oneshot::channel();
    ask.send(opened).await.unwrap();
    let mut ch = ch.await.unwrap().unwrap();
    let marker = home.path().join("started");
    let script = format!("touch {}; sleep 1; echo done", marker.display());
    let op = json!({"op": "exec", "box_id": 7, "env": [], "script": script, "timeout_secs": 30});
    ops::write_line(&mut ch, &op).await.unwrap();
    poll(|| marker.exists().then_some(())).await;

    old.tell.send(Frame::Relink {}).await.unwrap();
    let asked = Instant::now();
    let _new = tokio::time::timeout(Duration::from_secs(5), take(&listener))
        .await
        .expect("the agent dialed again");
    assert!(
        asked.elapsed() < Duration::from_secs(1),
        "dialed again after {:?}",
        asked.elapsed()
    );
    let reply = tokio::time::timeout(Duration::from_secs(10), ops::read_line(&mut ch))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ran(reply).output, "done\n");
    drop(ch);
    let why = tokio::time::timeout(Duration::from_secs(5), old_link)
        .await
        .expect("the agent closed the old link once its exec answered")
        .unwrap();
    assert_eq!(why, "the link closed");
    agent.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hello_while_the_controller_is_away_is_admitted_once_it_answers() {
    let dir = tempfile::tempdir().unwrap();
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    let links = Arc::new(Mutex::new(None));
    let socket = dir.path().join("controller.sock");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let agents = moor_machine_runner::Agents::new(&socket);
    let serving = agents.clone();
    tokio::spawn(async move { moor_machine_runner::serve_agents(listener, serving).await });
    let pubkey = key.public_key().to_openssh().unwrap();
    let (late, late_links) = (socket.clone(), Arc::clone(&links));
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        serve_controller(&late, pubkey, late_links);
    });
    let w = World {
        server,
        key,
        links,
        dir,
        agents,
    };
    let (_ws, _, answer) = handshake(&w, MACHINE, &w.key).await;
    assert!(matches!(answer, Frame::Welcome { .. }), "{answer:?}");
}
