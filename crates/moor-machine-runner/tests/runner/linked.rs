//! A machine whose agent links in: exec, hooks and relays reach it over the link, never ssh.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::rig::{Rig, HEADERS, MACHINE_ID};

/// Counts its runs in `~/prepared`, then prints the Box's device.
const PREPARE: &str =
    "echo \"$MOOR_BOX $MOOR_SLOT\" >> ~/prepared\necho '{\"device\":\"dev-'\"$MOOR_BOX\"'\"}'";

/// Notes each run and the device it was given in `~/released`.
const RELEASE: &str = "echo \"$MOOR_BOX $MOOR_FACT_DEVICE\" >> ~/released";

/// Notes each run and the device it was given in `~/suspended`.
const SUSPEND: &str = "echo \"$MOOR_BOX $MOOR_FACT_DEVICE\" >> ~/suspended";

/// The headers of Box 7 while it holds no machine.
const MISSING: [(&str, &str); 2] = [
    ("X-Moor-Box", "7"),
    ("X-Moor-Machine-Missing", "this Box holds no machine"),
];

struct Linked {
    rig: Rig,
}

impl Linked {
    fn start(args: &[&str]) -> Self {
        let mut all = vec!["--sweep-secs", "1"];
        all.extend_from_slice(args);
        let rig = Rig::with_args(&all);
        rig.hook("prepare", PREPARE);
        rig.hook("release", RELEASE);
        rig.hook("suspend", SUSPEND);
        let fake = env!("CARGO_BIN_EXE_fake-mcp");
        rig.hook("relays/fake", &format!("exec '{fake}'"));
        Self { rig }
    }

    fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        self.rig.call_with(tool, args, &HEADERS)
    }

    fn ssh_ran(&self) -> bool {
        self.rig.ssh_ran()
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
fn a_holder_only_request_is_misdirected_unless_this_runner_holds_the_link() {
    let m = Linked::start(&[]);
    let only = ("X-Moor-Holder-Only", "1");
    let held = [HEADERS.as_slice(), &[only]].concat();
    assert_eq!(m.rig.status(&held), 200);
    let mut other = held.clone();
    other[2] = ("X-Moor-Machine-Id", "8");
    assert_eq!(m.rig.status(&other), 421, "another runner holds machine 8");
    assert_eq!(
        m.rig.status(&other[..4]),
        200,
        "the Proxy's last try is served"
    );
}

#[test]
fn a_machine_whose_agent_dropped_its_link_is_reported_offline() {
    let mut m = Linked::start(&[]);
    m.rig.machine_agent.take().unwrap().shutdown_background();
    m.rig.online(false);
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
    let landed = std::fs::read_to_string(m.rig.tree.join("out/shot.png")).unwrap();
    assert_eq!(landed, "png");
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
    assert_eq!(m.rig.puts(), 0);
    assert!(!m.ssh_ran());
}

#[test]
fn a_relay_runs_the_machines_own_file_of_that_name_over_the_link() {
    let m = Linked::start(&["--relay", "fake", "--force-arg", "fake.udid=device"]);
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
    let held = json!({"state": "held", "machine": "minion", "machine_id": MACHINE_ID,
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

/// The machine service key a controller in another pod presents.
const KEY: &str = "machine-service-key";

/// The supervisor's held connection: its lines, and the stream that writes them.
type Carrier = BufReader<Box<dyn ReadWrite>>;

trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}

/// How the supervisor reaches the runner: the socket on this host, or across the network with
/// a key.
#[derive(Clone)]
enum Via {
    Local(PathBuf),
    Remote(u16, &'static str),
}

impl Via {
    /// A connection for `local`'s line on this host, or `hello`'s across the network.
    fn connect(&self, local: Option<Value>, hello: Value) -> Box<dyn ReadWrite> {
        match self {
            Self::Local(path) => {
                let mut conn = UnixStream::connect(path).unwrap();
                if let Some(line) = local {
                    writeln!(conn, "{line}").unwrap();
                }
                Box::new(conn)
            }
            Self::Remote(port, key) => {
                let mut conn = TcpStream::connect(("127.0.0.1", *port)).unwrap();
                let (name, mut v) = hello
                    .as_object()
                    .unwrap()
                    .clone()
                    .into_iter()
                    .next()
                    .unwrap();
                v["v"] = json!(1);
                v["key"] = json!(key);
                let hello = json!({ name: v });
                writeln!(conn, "{hello}").unwrap();
                Box::new(conn)
            }
        }
    }

    /// Asks the runner to forward Box `ports` of `box_id`, as the supervisor does; its answer and
    /// the carrier.
    fn forward(&self, box_id: i64, ports: &[u16]) -> (Value, Carrier) {
        let mut conn = self.connect(None, json!({"forwards": {}}));
        let req = json!({"box_id": box_id, "machine_id": MACHINE_ID, "ports": ports});
        writeln!(conn, "{req}").unwrap();
        let mut carrier = BufReader::new(conn);
        let mut line = String::new();
        carrier.read_line(&mut line).unwrap();
        (serde_json::from_str(&line).unwrap(), carrier)
    }

    /// Reads the next connection the carrier names; its number and Box port.
    fn next_conn(carrier: &mut Carrier) -> (u64, u16) {
        let mut line = String::new();
        carrier.read_line(&mut line).unwrap();
        let line: Value = serde_json::from_str(&line).unwrap();
        let conn = &line["conn"];
        let port = u16::try_from(conn["port"].as_u64().unwrap()).unwrap();
        (conn["n"].as_u64().unwrap(), port)
    }

    /// Dials back for connection `n`.
    fn dial_back(&self, n: u64) -> Box<dyn ReadWrite> {
        let local = json!({"conn": {"n": n}});
        self.connect(Some(local.clone()), local)
    }

    /// Answers every connection the carrier names as the Box would, echoing its bytes.
    fn echo(self, mut carrier: Carrier) {
        std::thread::spawn(move || loop {
            let (n, _) = Self::next_conn(&mut carrier);
            let mut back = self.dial_back(n);
            std::thread::spawn(move || {
                let mut got = [0u8; 4];
                while back.read_exact(&mut got).is_ok() && back.write_all(&got).is_ok() {}
            });
        });
    }
}

impl Linked {
    fn local(&self) -> Via {
        Via::Local(self.rig.dir().join("machine-forwards.sock"))
    }
}

/// Sends `ping` to the machine's loopback `port` and reads what comes back.
fn ping(port: u16) -> std::io::Result<String> {
    ping_at("127.0.0.1", port)
}

fn ping_at(ip: &str, port: u16) -> std::io::Result<String> {
    let mut conn = TcpStream::connect((ip, port))?;
    conn.set_read_timeout(Some(Duration::from_secs(5)))?;
    conn.write_all(b"ping")?;
    let mut got = [0u8; 4];
    conn.read_exact(&mut got)?;
    Ok(String::from_utf8_lossy(&got).into_owned())
}

#[test]
fn a_machine_connection_is_named_on_the_carrier_and_the_dial_back_pipes_it_both_ways() {
    let m = Linked::start(&[]);
    let via = m.local();
    let port = free_port();

    let (answer, mut carrier) = via.forward(7, &[port]);
    assert_eq!(answer, json!({"reply": "up"}));
    let machine = std::thread::spawn(move || ping(port));
    let (n, named) = Via::next_conn(&mut carrier);
    assert_eq!(named, port, "the carrier names the Box port");
    let mut back = via.dial_back(n);
    let mut got = [0u8; 4];
    back.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"ping", "the machine's bytes reach the Box");
    back.write_all(b"pong").unwrap();
    assert_eq!(
        machine.join().unwrap().unwrap(),
        "pong",
        "the Box's bytes reach the machine"
    );
    drop(carrier);
    eventually("the listener closed", || ping(port).is_err());
    assert!(!m.ssh_ran(), "the forward never used ssh");
}

#[test]
fn a_dial_back_across_the_network_with_the_wrong_key_is_refused() {
    let m = Linked::start(&["--links", "127.0.0.1:0", "--service-key", KEY]);
    let links = m.rig.links.unwrap();
    let (via, wrong) = (Via::Remote(links, KEY), Via::Remote(links, "not-the-key"));
    let port = free_port();

    let (answer, mut carrier) = via.forward(7, &[port]);
    assert_eq!(answer, json!({"reply": "up"}));
    let machine = std::thread::spawn(move || ping(port));
    let (n, _) = Via::next_conn(&mut carrier);
    let mut refused = wrong.dial_back(n);
    assert_eq!(
        refused.read(&mut [0u8; 4]).unwrap(),
        0,
        "the wrong key is hung up on"
    );
    let mut back = via.dial_back(n);
    let mut got = [0u8; 4];
    back.read_exact(&mut got).unwrap();
    back.write_all(&got).unwrap();
    assert_eq!(
        machine.join().unwrap().unwrap(),
        "ping",
        "the right key still claims it"
    );
}

#[test]
fn a_box_beyond_the_machines_slots_gets_no_listener() {
    let m = Linked::start(&[]);
    let via = m.local();
    let (first, second) = (free_port(), free_port());

    let (answer, _held) = via.forward(7, &[first]);
    assert_eq!(answer, json!({"reply": "up"}));
    let (answer, _) = via.forward(8, &[second]);
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

    let (answer, _) = m.local().forward(7, &[]);
    assert_eq!(answer["reply"], "refused");
}

#[test]
fn a_machine_port_answers_on_127_0_0_1_alone() {
    let m = Linked::start(&[]);
    let via = m.local();
    let port = free_port();

    let (answer, carrier) = via.forward(7, &[port]);
    assert_eq!(answer, json!({"reply": "up"}));
    via.echo(carrier);
    assert_eq!(ping(port).unwrap(), "ping");
    assert!(
        ping_at("127.0.0.2", port).is_err(),
        "no other address of the machine reaches the Box"
    );
}
