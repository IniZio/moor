//! The controller answers requests while the Box Proxies are still being restarted.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::io::Write as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, Intent, IntentChange, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::{BoxName, ProviderId};
use serde_json::{json, Value};

use super::fronts::ask;
use super::{config, env, Buf};
use crate::controller::{serve_started, start, Sink};
use crate::store::Store;
use crate::testing::clock::FakeClock;

const WAIT: Duration = Duration::from_secs(20);

/// A controller whose Proxy restart blocks until its gate is sent a message.
struct Slow {
    /// Holds the config and the state dir.
    _dir: tempfile::TempDir,
    main: PathBuf,
    log: Buf,
    /// Gets a message when the restart begins.
    entered: Receiver<()>,
    /// Lets the restart finish.
    gate: Sender<()>,
    /// Gets a message when the restart finishes.
    finished: Receiver<()>,
    wake: UnixStream,
    serving: JoinHandle<u8>,
}

/// Starts a controller whose Box 1 had a start cut off by a dead process.
fn slow() -> Slow {
    slow_after(Duration::ZERO)
}

/// [`slow`], with the Proxy restart waiting `after` once the controller starts.
fn slow_after(after: Duration) -> Slow {
    let dir = tempfile::tempdir().expect("tmp");
    let cfg = config(dir.path(), "");
    let running = start(&env(dir.path()), &cfg).expect("start");
    let main = running.socket.clone();
    cut_off_start(&dir);
    let log = Buf::default();
    let sink: Sink = Arc::new(Mutex::new(log.clone()));
    let (entered_tx, entered) = mpsc::channel();
    let (gate, gate_rx) = mpsc::channel::<()>();
    let (finished_tx, finished) = mpsc::channel();
    let (wake, signal) = UnixStream::pair().expect("wake pair");
    let serving = thread::spawn(move || {
        serve_started(running, &signal, &sink, after, move |_| {
            entered_tx.send(()).expect("entered");
            gate_rx.recv().expect("gate");
            finished_tx.send(()).expect("finished");
            vec!["slow proxy".to_owned()]
        })
    });
    Slow {
        _dir: dir,
        main,
        log,
        entered,
        gate,
        finished,
        wake,
        serving,
    }
}

/// Records Box 1 as provisioning under a process that no longer exists.
fn cut_off_start(dir: &tempfile::TempDir) {
    let mut child = moor_testkit::ChildGuard::new(Command::new("true").spawn().expect("spawn"));
    let dead = child.id();
    child.wait().expect("wait");
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let store = Store::open(&dir.path().join("state").join("moor.db"), clock).expect("store");
    let new = Decision::insert(NewBox {
        name: BoxName::parse_allow_bare("cut").expect("name"),
        repo: None,
    });
    let id = store.apply(new).expect("box").box_id;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
        ..Fields::default()
    };
    let starting = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields)
        .intent(IntentChange::Set(Intent::Provisioning { pid: dead }));
    store.apply(starting).expect("starting");
}

/// The response to `request`, or a failure when the controller does not answer in time.
fn answer(main: &Path, request: Value) -> Value {
    let (tx, rx) = mpsc::channel();
    let main = main.to_path_buf();
    thread::spawn(move || tx.send(ask(&main, &request)).ok());
    rx.recv_timeout(WAIT)
        .expect("the controller served no request while the Proxies restarted")
}

impl Slow {
    /// Lets the restart finish, signals the drain and waits for the controller to exit.
    fn finish(self) -> Buf {
        self.gate.send(()).ok();
        (&self.wake).write_all(b"x").expect("signal");
        assert_eq!(self.serving.join().expect("serve"), 0);
        self.log
    }
}

#[test]
fn a_request_is_answered_while_the_proxies_are_still_restarting() {
    let slow = slow();
    slow.entered.recv_timeout(WAIT).expect("restart began");
    let got = answer(&slow.main, json!({"cmd": "whoami"}));
    assert_eq!(got["class"], "ok", "{got}");
    assert!(slow.finished.try_recv().is_err(), "the restart was held");
    slow.finish();
}

#[test]
fn every_proxy_is_still_restarted_after_the_controller_starts() {
    let slow = slow();
    answer(&slow.main, json!({"cmd": "whoami"}));
    slow.gate.send(()).expect("release");
    slow.finished.recv_timeout(WAIT).expect("restart finished");
    let log = slow.finish();
    assert!(log.text().contains("proxy: slow proxy"), "{}", log.text());
}

#[test]
fn an_interrupted_start_is_failed_before_the_first_request_is_served() {
    let slow = slow();
    slow.entered.recv_timeout(WAIT).expect("restart began");
    let got = answer(&slow.main, json!({"cmd": "show", "box": "cut"}));
    assert_eq!(got["class"], "ok", "{got}");
    let shown = got["value"]["box"].to_string();
    assert!(shown.contains("setup_failed"), "{shown}");
    slow.finish();
}

#[test]
fn the_proxies_are_restarted_only_once_the_wait_after_start_is_over() {
    let after = Duration::from_millis(600);
    let slow = slow_after(after);
    assert!(
        slow.entered.recv_timeout(after / 2).is_err(),
        "the restart began before the wait was over"
    );
    slow.entered.recv_timeout(WAIT).expect("restart began");
    slow.finish();
}
