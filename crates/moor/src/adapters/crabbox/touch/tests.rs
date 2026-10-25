//! Lease touches: only real use sends a heartbeat; looking at a Box never touches its lease.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{BoxStep, Driver, DriverError};
use moor_core::seam::proc::{Deadline, Tool};
use tempfile::TempDir;

use crate::adapters::crabbox::Crabbox;
use crate::testing::clock::FakeClock;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::rng::FakeRng;

const SSH_LINE: &str = "ssh -i '/k/id' -o 'StrictHostKeyChecking=yes' -p '22' 'u@10.0.0.1'\n";

/// One host: a scripted crabbox and ssh, a clock and a state dir that outlive any driver.
struct Rig {
    p: Arc<ScriptedProc>,
    clock: Arc<FakeClock>,
    state: TempDir,
    /// When set, every ssh call fails as for a Box that moved.
    moved: Arc<AtomicBool>,
}

impl Rig {
    fn new() -> Self {
        let p = Arc::new(ScriptedProc::new());
        let moved = Arc::new(AtomicBool::new(false));
        let gone = Arc::clone(&moved);
        p.on(Tool::Crabbox, &["ssh"], Reply::Ok(SSH_LINE.to_owned()));
        p.on(Tool::Crabbox, &["heartbeat"], Reply::Ok(String::new()));
        p.on(
            Tool::Ssh,
            &[],
            Reply::Computed(Arc::new(move |_| {
                if gone.load(Ordering::SeqCst) {
                    Reply::Fail {
                        code: 255,
                        stdout: String::new(),
                        stderr: "ssh: connect to host 10.0.0.1 port 22: No route to host"
                            .to_owned(),
                    }
                } else {
                    Reply::Ok("x".to_owned())
                }
            })),
        );
        Self {
            p,
            clock: Arc::new(FakeClock::new(Ts::from_millis(1_000))),
            state: tempfile::tempdir().expect("state dir"),
            moved,
        }
    }

    /// A fresh driver, as a new moor process wires one.
    fn driver(&self) -> Crabbox {
        let sockets: PathBuf = self.state.path().join("s");
        let d = Crabbox::new(
            self.p.clone(),
            self.clock.clone(),
            Arc::new(FakeRng::default()),
        )
        .with_pinned_options()
        .with_state_dir(Some(self.state.path().to_owned()), sockets);
        let _ = d.version_ok.set(());
        let _ = d.no_probe.set(false);
        d
    }

    /// The first word of every crabbox call so far.
    fn verbs(&self) -> Vec<String> {
        self.p
            .calls()
            .into_iter()
            .filter(|c| c.tool == Tool::Crabbox)
            .filter_map(|c| c.args.first().cloned())
            .collect()
    }
}

fn handle() -> BoxHandle {
    BoxHandle::new("p/box")
}

fn path() -> BoxPath {
    BoxPath::under("/work", ".moor/agent-state.json").unwrap()
}

fn peek(d: &Crabbox) -> Result<Option<String>, DriverError> {
    d.peek_file(&handle(), &path())
}

#[test]
fn looking_at_a_box_reuses_its_saved_target_and_never_touches_the_lease() {
    let rig = Rig::new();
    peek(&rig.driver()).unwrap();
    rig.clock.sleep(Duration::from_hours(3));
    let d = rig.driver();
    peek(&d).unwrap();
    d.connect_argv(&handle()).unwrap();
    let _ = d.batch(
        &handle(),
        &[],
        &[BoxStep::Peek(path())],
        Deadline(Duration::from_secs(5)),
    );
    assert_eq!(rig.verbs(), ["ssh"]);
}

#[test]
fn real_use_sends_one_heartbeat_to_that_lease() {
    let rig = Rig::new();
    let d = rig.driver();
    d.put_file(&handle(), &path(), b"{}", 0o600).unwrap();
    d.touch(&handle()).unwrap();
    let beats: Vec<Vec<String>> = rig
        .p
        .calls()
        .into_iter()
        .filter(|c| c.tool == Tool::Crabbox && c.args.first().is_some_and(|a| a == "heartbeat"))
        .map(|c| c.args)
        .collect();
    assert_eq!(beats, [["heartbeat", "--provider", "p", "--id", "box"]]);
    rig.clock.sleep(Duration::from_mins(10));
    d.touch(&handle()).unwrap();
    assert_eq!(rig.verbs(), ["heartbeat", "ssh", "heartbeat"]);
}

#[test]
fn a_stale_target_falls_back_to_one_lookup_and_is_saved_again() {
    let rig = Rig::new();
    let d = rig.driver();
    peek(&d).unwrap();
    rig.moved.store(true, Ordering::SeqCst);
    assert!(peek(&d).is_err());
    rig.moved.store(false, Ordering::SeqCst);
    peek(&d).unwrap();
    peek(&d).unwrap();
    peek(&rig.driver()).unwrap();
    assert_eq!(rig.verbs(), ["ssh", "ssh"]);
}
