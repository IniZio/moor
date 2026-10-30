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

/// Notes each run and the device it was given in `~/suspended`.
const SUSPEND: &str = "#!/bin/sh\necho \"$MOOR_BOX $MOOR_FACT_DEVICE\" >> ~/suspended\n";

/// The headers of Box 7 while it holds no machine.
const MISSING: [(&str, &str); 2] = [
    ("X-Moor-Box", "7"),
    ("X-Moor-Machine-Missing", "this Box holds no machine"),
];

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
        std::fs::create_dir_all(hooks.join("relays")).unwrap();
        moor_testkit::write_executable(&hooks.join("prepare"), PREPARE).unwrap();
        moor_testkit::write_executable(&hooks.join("release"), RELEASE).unwrap();
        moor_testkit::write_executable(&hooks.join("suspend"), SUSPEND).unwrap();
        let fake = env!("CARGO_BIN_EXE_fake-mcp");
        let relay = format!("#!/bin/sh\nexec '{fake}'\n");
        moor_testkit::write_executable(&hooks.join("relays/fake"), &relay).unwrap();
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
    assert!(
        text.contains("device=dev-7"),
        "prepare's facts reach it: {out}"
    );
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
fn sync_lands_the_tree_in_the_box_copy_over_the_link_and_never_dials_ssh() {
    let m = Linked::start(&[]);
    m.rig.write("src/main.rs", "fn main() {}");
    m.rig.write("node_modules/x.js", "skip");
    m.call("sync", json!({"exclude": ["node_modules"]}))
        .unwrap();
    let copied = std::fs::read_to_string(m.rig.copy().join("src/main.rs")).unwrap();
    assert_eq!(copied, "fn main() {}");
    assert!(!m.rig.copy().join("node_modules").exists(), "excludes hold");
    std::fs::remove_file(m.rig.tree.join("src/main.rs")).unwrap();
    m.call("sync", json!({})).unwrap();
    assert!(
        !m.rig.copy().join("src/main.rs").exists(),
        "a deleted file goes"
    );
    assert!(!m.ssh_ran());
}

#[test]
fn sync_into_a_box_copy_that_is_a_symlink_out_is_refused() {
    let m = Linked::start(&[]);
    let outside = m.rig.dir().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::remove_dir_all(m.rig.copy()).unwrap();
    std::os::unix::fs::symlink(&outside, m.rig.copy()).unwrap();
    m.rig.write("a.txt", "a");
    let err = m.call("sync", json!({})).unwrap_err();
    assert!(err.contains("rsync"), "{err}");
    assert_eq!(
        std::fs::read_dir(&outside).unwrap().count(),
        0,
        "nothing landed"
    );
    assert!(!m.ssh_ran());
}

#[test]
fn pull_reads_a_file_in_the_box_copy_over_the_link() {
    let m = Linked::start(&[]);
    std::fs::write(m.rig.copy().join("shot.png"), "png").unwrap();
    let got = m
        .call("pull", json!({"path": "shot.png", "dest": "out/shot.png"}))
        .unwrap();
    assert_eq!(got, json!({"dest": "out/shot.png", "bytes": 3}));
    assert_eq!(m.rig.puts.lock().unwrap()[0].1, b"png");
    assert!(!m.ssh_ran());
}

#[test]
fn pull_of_a_path_outside_the_box_dirs_is_refused() {
    let m = Linked::start(&[]);
    let secret = m.rig.remote.join("secret");
    std::fs::write(&secret, "no").unwrap();
    std::os::unix::fs::symlink(&secret, m.rig.copy().join("link")).unwrap();
    let absolute = secret.display().to_string();
    for path in ["../../secret", absolute.as_str(), "link"] {
        let err = m
            .call("pull", json!({"path": path, "dest": "x"}))
            .unwrap_err();
        assert!(err.contains("outside the Box's dirs"), "{path}: {err}");
    }
    assert!(m.rig.puts.lock().unwrap().is_empty());
    assert!(!m.ssh_ran());
}

#[test]
fn a_relay_runs_the_machines_own_file_of_that_name_over_the_link() {
    let m = Linked::start(&["--relay", "fake=false", "--force-arg", "fake.udid=device"]);
    let out = m
        .call("fake_tap", json!({"udid": "other", "x": 3}))
        .unwrap();
    assert_eq!(out, json!({"udid": "dev-7", "x": 3}));
    let calls = lines(&m.rig.remote.join("fake-calls"));
    assert_eq!(calls, [r#"{"udid":"dev-7","x":3}"#]);
    assert!(!m.ssh_ran());
}

#[test]
fn an_asleep_box_runs_the_machines_suspend_hook_with_its_facts() {
    let m = Linked::start(&[]);
    m.call("exec", json!({"command": "true"})).unwrap();
    m.rig.asleep.lock().unwrap().push(7);
    let suspended = m.rig.remote.join("suspended");
    eventually("suspend ran", || lines(&suspended) == ["7 dev-7"]);
    assert!(!m.ssh_ran());
}

#[test]
fn a_claim_held_on_an_enrolled_machine_prepares_it_over_the_link() {
    let m = Linked::start(&[]);
    let held = json!({"state": "held", "machine": "minion", "ssh": "", "machine_id": MACHINE_ID,
        "slot": 0, "ports": [8081]});
    hold_answers(&m, vec![held]);
    let got = m
        .rig
        .call_with("machine_claim", json!({}), &MISSING)
        .unwrap();
    assert_eq!(got["facts"], json!({"device": "dev-7"}), "{got}");
    assert_eq!(lines(&m.rig.remote.join("prepared")), ["7 0"]);
    assert!(!m.ssh_ran());
}

#[test]
fn a_claim_on_a_removed_machine_is_refused_with_the_reason() {
    let m = Linked::start(&[]);
    let removed = json!({"state": "refused", "reason": "machine minion was removed"});
    hold_answers(&m, vec![removed]);
    let err = m
        .rig
        .call_with("machine_claim", json!({"machine": "minion"}), &MISSING)
        .unwrap_err();
    assert!(err.contains("machine minion was removed"), "{err}");
    assert!(lines(&m.rig.remote.join("prepared")).is_empty());
    assert!(!m.ssh_ran());
}

fn hold_answers(m: &Linked, answers: Vec<Value>) {
    m.rig.holds.lock().unwrap().answers = answers.into();
}

/// A free loopback port, unbound again.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Echoes every connection on a fresh loopback port, standing in for the tunnel into the Box.
fn echo() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut out = conn.try_clone().unwrap();
                let _ = std::io::copy(&mut &conn, &mut out);
            });
        }
    });
    port
}

impl Linked {
    /// Asks the runner to forward `ports` of `box_id`, as the supervisor does; its answer and
    /// the connection that holds the forwards.
    fn forward(
        &self,
        box_id: i64,
        ports: &[(u16, u16)],
    ) -> (Value, std::os::unix::net::UnixStream) {
        use std::io::{BufRead, Write};
        let path = self.rig.dir().join("machine-forwards.sock");
        let mut conn = std::os::unix::net::UnixStream::connect(path).unwrap();
        let req = json!({"box_id": box_id, "machine_id": MACHINE_ID, "ports": ports});
        writeln!(conn, "{req}").unwrap();
        let mut line = String::new();
        std::io::BufReader::new(&conn).read_line(&mut line).unwrap();
        (serde_json::from_str(&line).unwrap(), conn)
    }
}

/// Sends `ping` to the machine's loopback `port` and reads what comes back.
fn ping(port: u16) -> std::io::Result<String> {
    ping_at("127.0.0.1", port)
}

fn ping_at(ip: &str, port: u16) -> std::io::Result<String> {
    use std::io::{Read, Write};
    let mut conn = std::net::TcpStream::connect((ip, port))?;
    conn.set_read_timeout(Some(Duration::from_secs(5)))?;
    conn.write_all(b"ping")?;
    let mut got = [0u8; 4];
    conn.read_exact(&mut got)?;
    Ok(String::from_utf8_lossy(&got).into_owned())
}

#[test]
fn a_machine_port_reaches_the_box_until_the_supervisor_lets_go() {
    let m = Linked::start(&[]);
    let (machine_port, host_port) = (free_port(), echo());

    let (answer, conn) = m.forward(7, &[(machine_port, host_port)]);
    assert_eq!(answer, json!({"reply": "up"}));
    assert_eq!(ping(machine_port).unwrap(), "ping", "the Box answered");
    drop(conn);
    eventually("the listener closed", || ping(machine_port).is_err());
    assert!(!m.ssh_ran(), "the forward never used ssh");
}

#[test]
fn a_box_beyond_the_machines_slots_gets_no_listener() {
    let m = Linked::start(&[]);
    let (first, second) = (free_port(), free_port());
    let host_port = echo();

    let (answer, _held) = m.forward(7, &[(first, host_port)]);
    assert_eq!(answer, json!({"reply": "up"}));
    let (answer, _) = m.forward(8, &[(second, host_port)]);
    assert_eq!(answer["reply"], "refused");
    assert!(
        answer["reason"].as_str().unwrap().contains("slots"),
        "{answer}"
    );
    assert!(ping(second).is_err(), "nothing listens for the second Box");
}

#[test]
fn a_forward_of_no_ports_is_refused() {
    let m = Linked::start(&[]);

    let (answer, _) = m.forward(7, &[]);
    assert_eq!(answer["reply"], "refused");
}

#[test]
fn a_machine_port_answers_on_127_0_0_1_alone() {
    let m = Linked::start(&[]);
    let (machine_port, host_port) = (free_port(), echo());

    let (answer, _held) = m.forward(7, &[(machine_port, host_port)]);
    assert_eq!(answer, json!({"reply": "up"}));
    assert_eq!(ping(machine_port).unwrap(), "ping");
    assert!(
        ping_at("127.0.0.2", machine_port).is_err(),
        "no other address of the machine reaches the Box"
    );
}
