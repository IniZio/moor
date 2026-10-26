//! A runner process wired to a fake controller and a fake machine.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::controller::{ServerFrame, PROTOCOL};
use moor_core::response::{ErrorView, Response};
use moor_core::ErrorKind;
use serde_json::{json, Value};

/// Stands in for `ssh`: runs the remote command locally, in `$FAKE_REMOTES/<target>` when that
/// exists and else in `$FAKE_REMOTE_HOME`, the way sshd
/// hands it to the remote user's shell. As with sshd, the command is not this process: killing
/// it leaves the command running, and the command's stdin is a pipe that ends when this
/// process's own stdin does.
const FAKE_SSH: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-l|-p|-i|-F) shift 2 ;;
    --) shift; break ;;
    -*) shift ;;
    *) break ;;
  esac
done
home="$FAKE_REMOTE_HOME"
[ -d "$FAKE_REMOTES/$1" ] && home="$FAKE_REMOTES/$1"
shift
cd "$home" || exit 255
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

/// `box-put` requests with the bytes of their `src`.
pub type Puts = Arc<Mutex<Vec<(Value, Vec<u8>)>>>;

/// The machines' ssh targets `machine-targets` lists, or none to refuse it.
pub type Targets = Arc<Mutex<Option<Vec<String>>>>;

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
        let targets = Arc::new(Mutex::new(Some(vec!["moor@minion".to_owned()])));
        fake_controller(
            &dir.path().join("c.sock"),
            tree.clone(),
            Fake {
                open: Arc::clone(&open),
                asleep: Arc::clone(&asleep),
                mirror_excludes: Arc::clone(&mirror_excludes),
                puts: Arc::clone(&puts),
                targets: Arc::clone(&targets),
            },
        );
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let (runner, port) = spawn(dir.path(), &args);
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
        (self.runner, self.port) = spawn(self.dir.path(), &self.args);
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
        let (reply, _, progress) =
            self.post(&rpc(2, "tools/call", params), headers, session.as_deref());
        (Self::result(&reply), progress)
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
    ) -> (Value, Option<String>, Vec<String>) {
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
        let progress = messages
            .iter()
            .filter(|m| m["method"] == "notifications/progress")
            .map(|m| {
                m["params"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect();
        let json = messages
            .into_iter()
            .find(|v| v.get("id").is_some())
            .unwrap_or(Value::Null);
        (json, session, progress)
    }
}

/// Starts a runner in the rig at `dir`; it and the port it listens on.
fn spawn(dir: &Path, args: &[String]) -> (Child, u16) {
    let path = format!(
        "{}:{}",
        dir.join("bin").display(),
        std::env::var("PATH").unwrap()
    );
    let mut runner = Command::new(env!("CARGO_BIN_EXE_moor-machine-runner"))
        .args(["--listen", "127.0.0.1:0", "--socket"])
        .arg(dir.join("c.sock"))
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
    std::thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::sink()));
    (runner, port)
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

/// The `show` list reply: each Box in `open`, asleep when `asleep` names it.
fn show_value(open: &[i64], asleep: &[i64]) -> Value {
    let boxes: Vec<Value> = open
        .iter()
        .map(|id| {
            let lifecycle = if asleep.contains(id) {
                "asleep"
            } else {
                "ready"
            };
            json!({ "id": id, "lifecycle": lifecycle })
        })
        .collect();
    json!({ "boxes": boxes, "box": null })
}

/// Serves `box-mirror` the way the controller does, from `tree`, with a local rsync, and
/// `show` listing the Boxes in `open`.
/// What the fake controller shares with the test.
struct Fake {
    open: Arc<Mutex<Vec<i64>>>,
    asleep: Arc<Mutex<Vec<i64>>>,
    mirror_excludes: Arc<Mutex<Vec<Value>>>,
    puts: Puts,
    targets: Targets,
}

fn fake_controller(socket: &Path, tree: PathBuf, fake: Fake) {
    let Fake {
        open,
        asleep,
        mirror_excludes,
        puts,
        targets,
    } = fake;
    let listener = UnixListener::bind(socket).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let hello = ServerFrame::Hello { protocol: PROTOCOL };
            writeln!(writer, "{}", serde_json::to_string(&hello).unwrap()).unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            let frame: Value = serde_json::from_str(&line).unwrap();
            let req = request(&frame).unwrap();
            if req["cmd"] == "show" {
                let value = show_value(&open.lock().unwrap(), &asleep.lock().unwrap());
                reply(&mut writer, Response::ok("show", Some(value)));
                continue;
            }
            if req["cmd"] == "machine-targets" {
                reply(
                    &mut writer,
                    targets_envelope(targets.lock().unwrap().clone()),
                );
                continue;
            }
            if req["cmd"] == "box-put" {
                let bytes = std::fs::read(req["src"].as_str().unwrap()).unwrap();
                let value =
                    json!({"path": "/work/x", "bytes": bytes.len(), "confirmation": "wrote"});
                puts.lock().unwrap().push((req.clone(), bytes));
                reply(&mut writer, Response::ok("box-put", Some(value)));
                continue;
            }
            assert_eq!(req["cmd"], "box-mirror", "{frame}");
            assert_eq!(req["id"], 7, "{frame}");
            mirror_excludes.lock().unwrap().push(req["exclude"].clone());
            let dest = req["dest"].as_str().unwrap().to_owned();
            std::fs::create_dir_all(&dest).unwrap();
            let ok = Command::new("rsync")
                .args(["-a", "--delete", "--exclude=.git"])
                .arg(format!("{}/", tree.display()))
                .arg(format!("{dest}/"))
                .status()
                .unwrap()
                .success();
            assert!(ok, "fake controller rsync");
            let value = json!({"from": "/work", "to": dest, "files": 1, "bytes": 1,
                "confirmation": "mirrored"});
            let envelope = Response::ok("box-mirror", Some(value));
            reply(&mut writer, envelope);
        }
    });
}

/// Writes one response frame carrying `envelope`.
fn reply(writer: &mut impl Write, envelope: Response) {
    let frame = ServerFrame::Response {
        id: "1".to_owned(),
        envelope,
    };
    writeln!(writer, "{}", serde_json::to_string(&frame).unwrap()).unwrap();
}

/// `machine-targets` answered with `targets`, or failed when there are none.
fn targets_envelope(targets: Option<Vec<String>>) -> Response {
    match targets {
        Some(targets) => Response::ok("machine-targets", Some(json!({ "targets": targets }))),
        None => Response::err(
            "machine-targets",
            ErrorView {
                kind: ErrorKind::Internal,
                message: "the store is down".to_owned(),
                hints: Vec::new(),
                context: Vec::new(),
            },
        ),
    }
}

/// The request inside a client frame: the object that names a `cmd`.
fn request(v: &Value) -> Option<&Value> {
    match v {
        Value::Object(m) if m.contains_key("cmd") => Some(v),
        Value::Object(m) => m.values().find_map(request),
        _ => None,
    }
}
