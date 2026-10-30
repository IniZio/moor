//! A runner process wired to a fake controller and a fake machine.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

mod controller;
use controller::{fake_controller, Fake};

/// Stands in for `ssh`: runs the remote command locally, in `$FAKE_REMOTES/<target>` when that
/// exists and else in `$FAKE_REMOTE_HOME`, the way sshd
/// hands it to the remote user's shell. As with sshd, the command is not this process: killing
/// it leaves the command running, and the command's stdin is a pipe that ends when this
/// process's own stdin does.
const FAKE_SSH: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-l|-p|-i|-F) shift 2 ;;
    -R|-L) fwd="$fwd $1 $2"; shift 2 ;;
    -N) hold=1; shift ;;
    -G) cat "$HOME/.ssh-G" 2>/dev/null ||
          printf 'stricthostkeychecking yes\nuserknownhostsfile %s/.ssh/known_hosts\n' "$HOME"
        exit 0 ;;
    --) shift; break ;;
    -*) shift ;;
    *) break ;;
  esac
done
home="$FAKE_REMOTE_HOME"
[ -d "$FAKE_REMOTES/$1" ] && home="$FAKE_REMOTES/$1"
shift
cd "$home" || exit 255
if [ -n "$hold" ]; then
  echo "up$fwd" >> "$home/forwards"
  trap 'echo "down$fwd" >> "$home/forwards"; exit 0' TERM
  while :; do sleep 1 & wait $!; done
fi
fifo=$(mktemp -u "$home/.ssh-in.XXXXXX") && mkfifo "$fifo" || exit 255
exec 3<&0
cat <&3 >"$fifo" 3<&- &
exec 3<&-
HOME="$home" SHELL=/bin/sh /bin/sh -c "$*" <"$fifo"
code=$?
rm -f "$fifo"
exit "$code"
"#;

/// Stands in for `df -Pk`: reports the free KiB kept in `$FAKE_REMOTE_HOME/.free-kb`, plenty
/// when there is none, so a test sets the machine's disk by writing that file.
const FAKE_DF: &str = r#"#!/bin/sh
kb=$(cat "$FAKE_REMOTE_HOME/.free-kb" 2>/dev/null || echo 999999999)
echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
echo "/dev/fake 1000000000 1 $kb 1% $HOME"
"#;

/// The headers the Box's Proxy sets for Box 7 in slot 0 of machine `minion`.
pub const HEADERS: [(&str, &str); 4] = [
    ("X-Moor-Box", "7"),
    ("X-Moor-Machine", "minion"),
    ("X-Moor-Machine-Ssh", "moor@minion"),
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
        ("X-Moor-Machine-Ssh", "moor@minion"),
        ("X-Moor-Machine-Slot", "0"),
    ]
}

/// The enrolled id of machine `minion` when its agent links in.
pub const MACHINE_ID: i64 = 7;

/// The headers the Box's Proxy sets for Box 7 in slot 0 of `minion` once its agent links in.
pub const LINKED: [(&str, &str); 4] = [
    ("X-Moor-Box", "7"),
    ("X-Moor-Machine", "minion"),
    ("X-Moor-Machine-Id", "7"),
    ("X-Moor-Machine-Slot", "0"),
];

/// `box-put` requests with the bytes of their `src`.
pub type Puts = Arc<Mutex<Vec<(Value, Vec<u8>)>>>;

/// The machines' ssh targets `machine-targets` lists, or none to refuse it.
pub type Targets = Arc<Mutex<Option<Vec<String>>>>;

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
    /// The ssh targets the fake controller lists as enrolled; none refuses the listing.
    pub targets: Targets,
    /// The fake controller's `machine-hold`.
    pub holds: Arc<Mutex<FakeHolds>>,
    /// The fake controller's side of an agent link.
    pub link: FakeLink,
    /// The port the runner takes agent links on, when started with `--agents`.
    pub agents: Option<u16>,
    args: Vec<String>,
    port: u16,
    runner: Child,
    agent: ureq::Agent,
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
        let dir = moor_testkit::short_tempdir("mrun").unwrap();
        let bin = dir.path().join("bin");
        moor_testkit::write_host_guards(&bin).unwrap();
        moor_testkit::write_executable(&bin.join("ssh"), FAKE_SSH).unwrap();
        moor_testkit::write_executable(&bin.join("df"), FAKE_DF).unwrap();
        let tree = dir.path().join("box");
        let remote = dir.path().join("remote");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::create_dir_all(&remote).unwrap();
        let open = Arc::new(Mutex::new(vec![7]));
        let asleep = Arc::new(Mutex::new(Vec::new()));
        let mirror_excludes = Arc::new(Mutex::new(Vec::new()));
        let puts = Arc::new(Mutex::new(Vec::new()));
        // An enrolled machine has no ssh target, so a runner its agent links to lists none.
        let ssh_targets = if args.contains(&"--agents") {
            Vec::new()
        } else {
            vec!["moor@minion".to_owned()]
        };
        let targets = Arc::new(Mutex::new(Some(ssh_targets)));
        let holds = Arc::new(Mutex::new(FakeHolds::default()));
        let link = FakeLink::default();
        fake_controller(
            &dir.path().join("runner.sock"),
            tree.clone(),
            Fake {
                open: Arc::clone(&open),
                asleep: Arc::clone(&asleep),
                mirror_excludes: Arc::clone(&mirror_excludes),
                puts: Arc::clone(&puts),
                targets: Arc::clone(&targets),
                holds: Arc::clone(&holds),
                link: link.clone(),
            },
        );
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let (runner, port, agents) = spawn(dir.path(), &args);
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        Self {
            dir,
            tree,
            remote,
            open,
            asleep,
            mirror_excludes,
            puts,
            targets,
            holds,
            link,
            agents,
            args,
            port,
            runner,
            agent,
        }
    }

    /// Stops the runner and starts a new one with the same flags and state dir.
    pub fn restart(&mut self) {
        let _ = self.runner.kill();
        let _ = self.runner.wait();
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

    /// The home on machine `target`, which the fake controller now lists as enrolled.
    pub fn machine(&self, target: &str) -> PathBuf {
        let home = self.dir.path().join("remotes").join(target);
        std::fs::create_dir_all(&home).unwrap();
        let mut targets = self.targets.lock().unwrap();
        targets.get_or_insert_with(Vec::new).push(target.to_owned());
        home
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
            .agent
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
/// links on when `args` ask for one.
fn spawn(dir: &Path, args: &[String]) -> (Child, u16, Option<u16>) {
    let path = format!(
        "{}:{}",
        dir.join("bin").display(),
        std::env::var("PATH").unwrap()
    );
    let mut runner = Command::new(env!("CARGO_BIN_EXE_moor-machine-runner"))
        .args(["--listen", "127.0.0.1:0", "--socket"])
        .arg(dir.join("runner.sock"))
        .arg("--state-dir")
        .arg(dir.join("state"))
        .args(args)
        .env("PATH", path)
        .env("FAKE_REMOTE_HOME", dir.join("remote"))
        .env("FAKE_REMOTES", dir.join("remotes"))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(runner.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let port = line.trim().rsplit(':').next().unwrap().parse().unwrap();
    let agents = args.iter().any(|a| a == "--agents").then(|| {
        line.clear();
        stderr.read_line(&mut line).unwrap();
        line.trim().rsplit(':').next().unwrap().parse().unwrap()
    });
    std::thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::sink()));
    (runner, port, agents)
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.runner.kill();
        let _ = self.runner.wait();
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
