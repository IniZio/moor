//! A runner process wired to a fake controller and a fake machine.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

mod controller;
mod machine;
use controller::{fake_controller, Fake};

/// Starts the fake controller on the rig at `dir`, listing `open` as open, or refusing to list
/// when `listing_fails`; what it shares with the test.
fn start_controller(dir: &Path, tree: &Path, listing_fails: bool, open: &[i64]) -> Fake {
    let fake = Fake {
        open: Arc::new(Mutex::new(open.to_vec())),
        asleep: Arc::new(Mutex::new(Vec::new())),
        mirror_excludes: Arc::new(Mutex::new(Vec::new())),
        puts: Arc::new(Mutex::new(Vec::new())),
        holds: Arc::new(Mutex::new(FakeHolds::default())),
        link: FakeLink::default(),
        show_fails: Arc::new(AtomicBool::new(listing_fails)),
        shows_refused: Arc::new(AtomicUsize::new(0)),
    };
    fake_controller(&dir.join("runner.sock"), tree.to_owned(), fake.clone());
    fake
}

/// The enrolled id of machine `minion`, whose agent links in.
pub const MACHINE_ID: i64 = 7;

/// The headers the Box's Proxy sets for Box 7 in slot 0 of `minion`.
pub const HEADERS: [(&str, &str); 4] = [
    ("X-Moor-Box", "7"),
    ("X-Moor-Machine", "minion"),
    ("X-Moor-Machine-Id", "7"),
    ("X-Moor-Machine-Slot", "0"),
];

/// The headers for Box `id` on machine `machine`, with its copy made on the fake machine.
pub fn caller(
    rig: &Rig,
    id: &'static str,
    machine: &'static str,
) -> [(&'static str, &'static str); 4] {
    std::fs::create_dir_all(rig.remote.join("moor").join(id)).unwrap();
    [
        ("X-Moor-Box", id),
        ("X-Moor-Machine", machine),
        ("X-Moor-Machine-Id", "7"),
        ("X-Moor-Machine-Slot", "0"),
    ]
}

/// `box-put` requests with the bytes of their `src`.
pub type Puts = Arc<Mutex<Vec<(Value, Vec<u8>)>>>;

/// The fake controller's `machine-hold`: the answers it gives claims, in turn, the last one
/// again and again, and each request it served, with whether `~/suspended` existed by then.
#[derive(Default)]
pub struct FakeHolds {
    pub answers: std::collections::VecDeque<Value>,
    pub seen: Vec<Value>,
    held: bool,
}

pub struct Rig {
    dir: tempfile::TempDir,
    /// The fake Box's work tree, which the fake controller mirrors.
    pub tree: PathBuf,
    /// The fake machine user's home.
    pub remote: PathBuf,
    /// The Box ids the fake controller lists as open.
    pub open: Arc<Mutex<Vec<i64>>>,
    /// The open Box ids the fake controller reports asleep.
    pub asleep: Arc<Mutex<Vec<i64>>>,
    /// The `exclude` of each `box-mirror` request the fake controller served.
    pub mirror_excludes: Arc<Mutex<Vec<Value>>>,
    /// Each `box-put` request the fake controller served, with the bytes of its `src`.
    pub puts: Puts,
    /// The fake controller's `machine-hold`.
    pub holds: Arc<Mutex<FakeHolds>>,
    /// The fake controller's side of an agent link.
    pub link: FakeLink,
    /// While set, the fake controller refuses `show`, so the open Boxes cannot be listed.
    pub show_fails: Arc<AtomicBool>,
    /// How many `show` requests the fake controller refused.
    pub shows_refused: Arc<AtomicUsize>,
    /// The port the runner takes agent links on.
    pub agents: u16,
    /// The fake machine's agent, linked while it runs.
    pub machine_agent: Option<tokio::runtime::Runtime>,
    args: Vec<String>,
    port: u16,
    runner: Runner,
    http: ureq::Agent,
}

impl Rig {
    pub fn start() -> Self {
        Self::with_args(&[])
    }

    /// A runner started with one `--resource` flag per entry, such as `build=1`.
    pub fn with_resources(resources: &[&str]) -> Self {
        let args: Vec<&str> = resources.iter().flat_map(|r| ["--resource", r]).collect();
        Self::with_args(&args)
    }

    /// A runner started with `args` besides the ones every rig passes.
    pub fn with_args(args: &[&str]) -> Self {
        Self::build(
            args,
            false,
            &[7],
            moor_machine_agent::Machine::from_env().sweep_grace,
        )
    }

    /// A runner whose controller refuses to list the open Boxes from the start, with a closed
    /// Box 9's folders already on the machine. This and [`Self::with_open`] sweep folders of any
    /// age; every other rig only folders older than the agent's grace, so none of its own.
    pub fn listing_fails() -> Self {
        Self::build(&[], true, &[7], Duration::ZERO)
    }

    /// A runner whose controller lists the Boxes `open` as open from the start, so no first
    /// sweep can have listed fewer.
    pub fn with_open(open: &[i64]) -> Self {
        Self::build(&[], false, open, Duration::ZERO)
    }

    fn build(args: &[&str], listing_fails: bool, open: &[i64], sweep_grace: Duration) -> Self {
        let dir = moor_testkit::short_tempdir("mrun").unwrap();
        let bin = dir.path().join("bin");
        moor_testkit::write_host_guards(&bin).unwrap();
        moor_testkit::write_executable(&bin.join("ssh"), machine::NO_SSH).unwrap();
        let tree = dir.path().join("box");
        let remote = dir.path().join("remote");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::create_dir_all(remote.join("moor/7")).unwrap();
        if listing_fails {
            std::fs::create_dir_all(remote.join("moor/9")).unwrap();
            std::fs::create_dir_all(remote.join("moor-state/9")).unwrap();
        }
        std::fs::create_dir_all(machine::hooks_dir(dir.path())).unwrap();
        let fake = start_controller(dir.path(), &tree, listing_fails, open);
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let (runner, port, agents) = spawn(dir.path(), &args);
        let machine_agent =
            machine::start_agent(dir.path(), &remote, agents, &fake.link, sweep_grace);
        let http = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        let Fake {
            open,
            asleep,
            mirror_excludes,
            puts,
            holds,
            link,
            show_fails,
            shows_refused,
        } = fake;
        let rig = Self {
            dir,
            tree,
            remote,
            open,
            asleep,
            mirror_excludes,
            puts,
            holds,
            link,
            show_fails,
            shows_refused,
            agents,
            machine_agent: Some(machine_agent),
            args,
            port,
            runner,
            http,
        };
        rig.online(true);
        rig
    }

    /// Stops the runner and starts a new one with the same flags and state dir.
    pub fn restart(&mut self) {
        self.runner = Runner(None);
        (self.runner, self.port, self.agents) = spawn(self.dir.path(), &self.args);
    }

    /// The rig's own dir.
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// The runner's state dir.
    pub fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    /// Writes the fake machine's hook file `name`, such as `prepare` or `relays/fake`, running
    /// `script`.
    pub fn hook(&self, name: &str, script: &str) {
        let path = machine::hooks_dir(self.dir()).join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        moor_testkit::write_executable(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    }

    /// Waits until the runner has told the controller the machine is `online`.
    pub fn online(&self, online: bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let links = self.link.links.lock().unwrap().clone();
            if links.is_some_and(|l| l[0]["machine_id"] == MACHINE_ID && l[0]["online"] == online) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting: online={online}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Whether anything ran `ssh`.
    pub fn ssh_ran(&self) -> bool {
        self.dir().join("bin/ssh.ran").exists()
    }

    /// Sets the free space the fake machine reports, in KiB.
    pub fn set_free_kb(&self, kb: u64) {
        std::fs::write(self.remote.join(".free-kb"), kb.to_string()).unwrap();
    }

    /// The Box's copy on the fake machine.
    pub fn copy(&self) -> PathBuf {
        self.remote.join("moor/7")
    }

    pub fn write(&self, rel: &str, contents: &str) {
        let path = self.tree.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// Calls `tool` as Box 7; the tool's structured result, or the error's message.
    pub fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        self.call_with(tool, args, &HEADERS)
    }

    pub fn call_with(
        &self,
        tool: &str,
        args: Value,
        headers: &[(&str, &str)],
    ) -> Result<Value, String> {
        self.call_progress(tool, args, headers).0
    }

    /// Calls `tool` asking for progress; its result and the progress messages sent before it.
    pub fn call_progress(
        &self,
        tool: &str,
        args: Value,
        headers: &[(&str, &str)],
    ) -> (Result<Value, String>, Vec<String>) {
        let (init, session, _) = self.post(&rpc(1, "initialize", initialize()), headers, None);
        assert!(init.get("result").is_some(), "initialize: {init}");
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        self.post(&note, headers, session.as_deref());
        let params = json!({"name": tool, "arguments": args, "_meta": {"progressToken": "p"}});
        let (reply, _, notes) =
            self.post(&rpc(2, "tools/call", params), headers, session.as_deref());
        let progress = notes
            .iter()
            .filter(|m| m["method"] == "notifications/progress")
            .map(|m| {
                m["params"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            });
        (Self::result(&reply), progress.collect())
    }

    /// The whole JSON-RPC reply to a `tools/call` with `headers`.
    pub fn call_reply(&self, tool: &str, args: Value, headers: &[(&str, &str)]) -> Value {
        let (_, session, _) = self.post(&rpc(1, "initialize", initialize()), headers, None);
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        self.post(&note, headers, session.as_deref());
        let params = json!({"name": tool, "arguments": args});
        let (reply, _, _) = self.post(&rpc(2, "tools/call", params), headers, session.as_deref());
        reply
    }

    /// The runner's answer to `initialize`.
    pub fn initialized(&self) -> Value {
        let (init, _, _) = self.post(&rpc(1, "initialize", initialize()), &HEADERS, None);
        init["result"].clone()
    }

    /// The tools the runner lists to the caller with `headers`.
    pub fn tools(&self, headers: &[(&str, &str)]) -> Vec<Value> {
        let (init, session, _) = self.post(&rpc(1, "initialize", initialize()), headers, None);
        assert!(init.get("result").is_some(), "initialize: {init}");
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        self.post(&note, headers, session.as_deref());
        let (reply, _, _) = self.post(
            &rpc(2, "tools/list", json!({})),
            headers,
            session.as_deref(),
        );
        reply["result"]["tools"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("{reply}"))
    }

    /// Calls `tool`; its result and the methods of the notifications sent before it.
    pub fn call_watching(
        &self,
        tool: &str,
        args: Value,
        headers: &[(&str, &str)],
    ) -> (Result<Value, String>, Vec<String>) {
        let params = json!({"name": tool, "arguments": args});
        let (reply, _, notes) = self.post(&rpc(2, "tools/call", params), headers, None);
        let methods = notes
            .iter()
            .map(|m| m["method"].as_str().unwrap().to_owned());
        (Self::result(&reply), methods.collect())
    }

    /// Sends a `tools/call` as Box 7 without reading the reply; dropping the stream hangs up.
    pub fn send_call(&self, tool: &str, args: Value) -> std::net::TcpStream {
        let body = rpc(2, "tools/call", json!({"name": tool, "arguments": args})).to_string();
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\nContent-Length: {}\r\n",
            body.len()
        )
        .unwrap();
        for (k, v) in HEADERS {
            write!(stream, "{k}: {v}\r\n").unwrap();
        }
        write!(stream, "\r\n{body}").unwrap();
        stream
    }

    fn result(reply: &Value) -> Result<Value, String> {
        if let Some(e) = reply.get("error") {
            return Err(e["message"].as_str().unwrap().to_owned());
        }
        let result = &reply["result"];
        if result["isError"] == true {
            return Err(result["content"][0]["text"].as_str().unwrap().to_owned());
        }
        Ok(result["structuredContent"].clone())
    }

    fn post(
        &self,
        body: &Value,
        headers: &[(&str, &str)],
        session: Option<&str>,
    ) -> (Value, Option<String>, Vec<Value>) {
        let mut req = self
            .http
            .post(format!("http://127.0.0.1:{}/mcp", self.port))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        if let Some(s) = session {
            req = req.header("Mcp-Session-Id", s);
        }
        let mut resp = req.send(body.to_string()).unwrap();
        let session = resp
            .headers()
            .get("mcp-session-id")
            .map(|v| v.to_str().unwrap().to_owned());
        let text = resp.body_mut().read_to_string().unwrap();
        let messages: Vec<Value> = text
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(str::trim)
            .chain(std::iter::once(text.trim()))
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect();
        let (replies, notes): (Vec<Value>, Vec<Value>) =
            messages.into_iter().partition(|v| v.get("id").is_some());
        let json = replies.into_iter().next().unwrap_or(Value::Null);
        (json, session, notes)
    }
}

/// Starts a runner in the rig at `dir`; it, the port it listens on, and the one it takes agent
/// links on.
fn spawn(dir: &Path, args: &[String]) -> (Runner, u16, u16) {
    let path = format!(
        "{}:{}",
        dir.join("bin").display(),
        std::env::var("PATH").unwrap()
    );
    // The runner reports its links on each sweep, so the rig sweeps every second by default.
    let sweep: &[&str] = if args.iter().any(|a| a == "--sweep-secs") {
        &[]
    } else {
        &["--sweep-secs", "1"]
    };
    let child = Command::new(env!("CARGO_BIN_EXE_moor-machine-runner"))
        .args(["--listen", "127.0.0.1:0", "--socket"])
        .arg(dir.join("runner.sock"))
        .arg("--state-dir")
        .arg(dir.join("state"))
        .args(["--agents", "127.0.0.1:0"])
        .args(sweep)
        .args(args)
        .env("PATH", path)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut runner = Runner(Some(child));
    let stderr = runner.0.as_mut().and_then(|c| c.stderr.take());
    let mut stderr = BufReader::new(stderr.unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let port = line.trim().rsplit(':').next().unwrap().parse().unwrap();
    line.clear();
    stderr.read_line(&mut line).unwrap();
    let agents = line.trim().rsplit(':').next().unwrap().parse().unwrap();
    std::thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::sink()));
    (runner, port, agents)
}

/// A runner process, killed when dropped, so a rig that panics part-way never leaves it behind.
struct Runner(Option<Child>);

impl Drop for Runner {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn rpc(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn initialize() -> Value {
    json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": "rig", "version": "0"}
    })
}

/// The fake controller's side of an agent link: the enrolled key `machine-keys` lists, if any,
/// and the last `machine-links` the runner sent.
#[derive(Clone, Default)]
pub struct FakeLink {
    pub pubkey: Arc<Mutex<Option<String>>>,
    pub links: Arc<Mutex<Option<Value>>>,
}
