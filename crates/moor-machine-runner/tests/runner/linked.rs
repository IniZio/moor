//! A machine whose agent links in: exec, hooks and relays reach it over the link, never ssh.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use moor_machine_agent::{run, Machine, State};
use serde_json::{json, Value};
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};

use crate::rig::{Rig, LINKED, MACHINE_ID};

/// Stands in for `ssh` and leaves a mark beside itself, so a test can tell it was never run.
const NO_SSH: &str = "#!/bin/sh\ntouch \"$0.ran\"\nexit 255\n";

/// Counts its runs in `~/prepared`, then prints the Box's device.
const PREPARE: &str = "#!/bin/sh\necho \"$MOOR_BOX $MOOR_SLOT\" >> ~/prepared\necho '{\"device\":\"dev-'\"$MOOR_BOX\"'\"}'\n";

/// Notes each run and the device it was given in `~/released`.
const RELEASE: &str = "#!/bin/sh\necho \"$MOOR_BOX $MOOR_FACT_DEVICE\" >> ~/released\n";

struct Linked {
    rig: Rig,
    agent: Option<tokio::runtime::Runtime>,
}

impl Linked {
    fn start(args: &[&str]) -> Self {
        let mut all = vec!["--agents", "127.0.0.1:0", "--sweep-secs", "1"];
        all.extend_from_slice(args);
        let rig = Rig::with_args(&all);
        moor_testkit::write_executable(&rig.dir().join("bin/ssh"), NO_SSH).unwrap();
        let hooks = rig.dir().join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        moor_testkit::write_executable(&hooks.join("prepare"), PREPARE).unwrap();
        moor_testkit::write_executable(&hooks.join("release"), RELEASE).unwrap();
        std::fs::create_dir_all(rig.copy()).unwrap();
        let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
        *rig.link.pubkey.lock().unwrap() = Some(key.public_key().to_openssh().unwrap());
        let state_dir = rig.dir().join("agent");
        std::fs::create_dir_all(&state_dir).unwrap();
        let state = State {
            server: format!("http://127.0.0.1:{}", rig.agents.unwrap()),
            machine_id: MACHINE_ID,
            owner: "ada@example.com".to_owned(),
            name: "minion".to_owned(),
            slots: 1,
        };
        std::fs::write(
            state_dir.join("state.json"),
            serde_json::to_string(&state).unwrap(),
        )
        .unwrap();
        let text = key.to_openssh(LineEnding::LF).unwrap();
        std::fs::write(state_dir.join("id_ed25519"), text.as_bytes()).unwrap();
        let machine = Machine {
            home: rig.remote.clone(),
            shell: PathBuf::from("/bin/sh"),
            hooks,
        };
        let agent = tokio::runtime::Runtime::new().unwrap();
        agent.spawn(async move { run(&state_dir, machine, &|_| {}).await });
        let linked = Self {
            rig,
            agent: Some(agent),
        };
        linked.online(true);
        linked
    }

    /// Waits until the runner has told the controller the machine is `online`.
    fn online(&self, online: bool) {
        eventually(&format!("online={online}"), || {
            let links = self.rig.link.links.lock().unwrap().clone();
            links.is_some_and(|l| l[0]["machine_id"] == MACHINE_ID && l[0]["online"] == online)
        });
    }

    fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        self.rig.call_with(tool, args, &LINKED)
    }

    fn ssh_ran(&self) -> bool {
        self.rig.dir().join("bin/ssh.ran").exists()
    }
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn exec_runs_in_the_box_copy_over_the_link_and_never_dials_ssh() {
    let m = Linked::start(&[]);
    let out = m
        .call(
            "exec",
            json!({"command": "pwd; echo device=$MOOR_FACT_DEVICE"}),
        )
        .unwrap();
    let text = out["output"].as_str().unwrap();
    assert!(text.contains("moor/7"), "ran in the Box's copy: {out}");
    assert!(text.contains("device=dev-7"), "prepare's facts reach it: {out}");
    assert!(!m.ssh_ran(), "no ssh for a linked machine");
}

#[test]
fn prepare_runs_once_by_name_and_machine_info_returns_its_facts() {
    let m = Linked::start(&[]);
    let info = m.call("machine_info", json!({})).unwrap();
    assert_eq!(info["facts"], json!({"device": "dev-7"}), "{info}");
    m.call("exec", json!({"command": "true"})).unwrap();
    assert_eq!(lines(&m.rig.remote.join("prepared")), ["7 0"]);
    assert!(!m.ssh_ran());
}

#[test]
fn release_runs_on_the_machine_with_the_facts_once_the_box_closes() {
    let m = Linked::start(&[]);
    m.call("exec", json!({"command": "true"})).unwrap();
    m.rig.open.lock().unwrap().clear();
    let released = m.rig.remote.join("released");
    eventually("release ran", || lines(&released) == ["7 dev-7"]);
    eventually("the copy is gone", || !m.rig.copy().exists());
    assert!(!m.ssh_ran());
}

#[test]
fn a_machine_whose_agent_dropped_its_link_is_reported_offline() {
    let mut m = Linked::start(&[]);
    m.agent.take().unwrap().shutdown_background();
    m.online(false);
    let err = m.call("exec", json!({"command": "true"})).unwrap_err();
    assert!(err.contains("machine minion is offline"), "{err}");
    assert!(!m.ssh_ran());
}

#[test]
fn sync_to_a_linked_machine_is_refused_without_dialing_ssh() {
    let m = Linked::start(&[]);
    let err = m.call("sync", json!({})).unwrap_err();
    assert!(err.contains("not available yet"), "{err}");
    assert!(!m.ssh_ran());
}
