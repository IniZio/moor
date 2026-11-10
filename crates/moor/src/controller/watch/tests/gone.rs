//! A following watch ends once nobody reads it: its client hung up, or vanished and the
//! keepalive write fails.

use std::io::BufReader;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;

use super::*;
use crate::controller::conn::Peer;
use crate::controller::drain::Admitted;
use crate::controller::hangup::Hangup;
use crate::controller::watch::live;
use crate::vault::Vault;

/// Polls `done` until it holds or `within` passes; says whether it held.
fn eventually(within: Duration, done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    done()
}

/// The operator on the controller socket `server`, with the hangup watch the controller gives
/// each connection.
fn peer(dir: &Path, server: &UnixStream) -> Peer {
    std::fs::create_dir_all(dir).expect("vault dir");
    let vault = Vault::load_or_create(&dir.join("vault.key")).expect("vault");
    Peer {
        caller: Ok(moor_core::user::Caller::Operator(1000)),
        vault: Arc::new(vault),
        hangup: Hangup::of(server).ok(),
        conn: Admitted::unwatched(),
    }
}

/// Serves one connection on a thread; the receiver hears when it ends.
fn serve(dir: &tempfile::TempDir, server: UnixStream) -> Receiver<()> {
    let env = env(dir);
    let peer = peer(&dir.path().join("peer"), &server);
    let (done, ended) = channel();
    thread::spawn(move || {
        let reader = BufReader::new(server.try_clone().expect("clone"));
        handle(reader, server, &env, &peer);
        done.send(()).ok();
    });
    ended
}

/// Opens a follow on `client` for a Box that has no events.
fn follow_nothing(client: &mut UnixStream) {
    for frame in [
        json!({"frame": "hello", "protocol": moor_core::controller::PROTOCOL}),
        json!({"frame": "watch", "since": 0, "boxes": ["quiet"]}),
    ] {
        writeln!(client, "{frame}").expect("send");
    }
}

/// Events keep coming for Box `id`, none of which the follow matches.
fn keep_appending(store: &Store, id: BoxId, ended: &Receiver<()>) -> bool {
    for _ in 0..30 {
        append(store, id, 1);
        if ended.recv_timeout(Duration::from_millis(100)).is_ok() {
            return true;
        }
    }
    false
}

/// The client hangs up as `close` does; the follow, which never wrote, still ends and its
/// poller is gone.
fn ends_after(close: impl FnOnce(UnixStream)) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let id = add(&store, "alpha");
    append(&store, id, 1);
    let (server, mut client) = UnixStream::pair().expect("pair");
    let ended = serve(&dir, server);
    follow_nothing(&mut client);
    assert!(
        eventually(Duration::from_secs(5), || live() == 1),
        "the follow never started"
    );
    close(client);
    assert!(
        keep_appending(&store, id, &ended),
        "the follow still runs 3 s after its client hung up, inside one keepalive"
    );
    assert!(
        eventually(Duration::from_secs(2), || live() == 0),
        "the follow's poller still runs: {} live",
        live()
    );
}

#[test]
fn an_idle_follow_ends_once_its_client_closes_the_socket() {
    ends_after(drop);
}

#[test]
fn an_idle_follow_ends_once_its_client_shuts_the_socket_as_the_gate_does() {
    // The gate's stop shuts both sides and keeps the socket until its reader thread ends.
    let (keep, kept) = channel();
    ends_after(move |client| {
        client.shutdown(Shutdown::Both).expect("shutdown");
        keep.send(client).ok();
    });
    drop(kept);
}

#[test]
fn a_quiet_follow_sends_caught_up_as_a_keepalive_and_ends_when_it_cannot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let id = add(&store, "alpha");
    append(&store, id, 3);
    let last = *ids(&store).last().expect("an event");
    // A client that vanished: the writes fail once the second keepalive is written.
    let (mut probe, out) = Probe::new(2);
    let limits = Limits {
        keepalive: Duration::from_millis(100),
        ..fast(16)
    };
    let env = env(&dir);
    let (done, ended) = channel();
    thread::spawn(move || {
        stream(
            &mut probe,
            &env,
            ask(0, only_boxes(&["quiet"]), true),
            limits,
        );
        done.send(()).ok();
    });
    ended
        .recv_timeout(Duration::from_secs(5))
        .expect("the follow kept running after its writes failed");
    let got = frames(&out);
    assert_eq!(got.len(), 2, "{got:?}");
    for frame in &got {
        assert_eq!(frame["frame"], "caught-up", "{got:?}");
        assert_eq!(frame["cursor"], last, "{got:?}");
    }
    assert!(
        eventually(Duration::from_secs(2), || live() == 0),
        "the poller still runs: {} live",
        live()
    );
}

#[test]
fn a_follow_with_events_to_send_sends_no_keepalive_ahead_of_them() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let id = add(&store, "alpha");
    append(&store, id, 2);
    let (mut probe, out) = Probe::new(usize::MAX);
    let limits = Limits {
        keepalive: Duration::from_millis(50),
        ..fast(16)
    };
    let env = env(&dir);
    let shared = Arc::new(Shared::default());
    let stopper = Arc::clone(&shared);
    let streaming = thread::spawn(move || {
        run(
            &mut probe,
            &env,
            ask(0, everything(), true),
            limits,
            &shared,
        );
    });
    let all = ids(&store);
    assert!(eventually(Duration::from_secs(5), || frames(&out)
        .iter()
        .any(|f| f["frame"] == "caught-up")));
    stopper.gone.cancel();
    streaming.join().expect("stream");
    let got = frames(&out);
    let first_keepalive = got
        .iter()
        .position(|f| f["frame"] == "caught-up")
        .expect("a keepalive");
    assert_eq!(cursors(&got[..first_keepalive]), all, "{got:?}");
    assert_eq!(got[first_keepalive]["cursor"], *all.last().expect("events"));
}
