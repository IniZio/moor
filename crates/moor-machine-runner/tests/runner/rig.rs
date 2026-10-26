//! A runner process wired to a fake controller and a fake machine.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::controller::{ServerFrame, PROTOCOL};
use moor_core::response::Response;
use serde_json::{json, Value};

/// Stands in for `ssh`: runs the remote command locally, in `$FAKE_REMOTE_HOME`, the way sshd
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
shift
cd "$FAKE_REMOTE_HOME" || exit 255
fifo=$(mktemp -u "$FAKE_REMOTE_HOME/.ssh-in.XXXXXX") && mkfifo "$fifo" || exit 255
exec 3<&0
cat <&3 >"$fifo" 3<&- &
exec 3<&-
HOME="$FAKE_REMOTE_HOME" SHELL=/bin/sh /bin/sh -c "$*" <"$fifo"
code=$?
rm -f "$fifo"
exit "$code"
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

pub struct Rig {
    dir: tempfile::TempDir,
    /// The fake Box's work tree, which the fake controller mirrors.
    pub tree: PathBuf,
    /// The fake machine user's home.
    pub remote: PathBuf,
    /// The Box ids the fake controller lists as open.
    pub open: Arc<Mutex<Vec<i64>>>,
    /// The `exclude` of each `box-mirror` request the fake controller served.
    pub mirror_excludes: Arc<Mutex<Vec<Value>>>,
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
        let tree = dir.path().join("box");
        let remote = dir.path().join("remote");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::create_dir_all(&remote).unwrap();
        let open = Arc::new(Mutex::new(vec![7]));
        let mirror_excludes = Arc::new(Mutex::new(Vec::new()));
        fake_controller(
            &dir.path().join("c.sock"),
            tree.clone(),
            Arc::clone(&open),
            Arc::clone(&mirror_excludes),
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
            mirror_excludes,
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

/// Serves `box-mirror` the way the controller does, from `tree`, with a local rsync, and
/// `show` listing the Boxes in `open`.
fn fake_controller(
    socket: &Path,
    tree: PathBuf,
    open: Arc<Mutex<Vec<i64>>>,
    mirror_excludes: Arc<Mutex<Vec<Value>>>,
) {
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
                let ids = open.lock().unwrap().clone();
                let boxes: Vec<Value> = ids.iter().map(|id| json!({ "id": id })).collect();
                let value = json!({ "boxes": boxes, "box": null });
                let frame = ServerFrame::Response {
                    id: "1".to_owned(),
                    envelope: Response::ok("show", Some(value)),
                };
                writeln!(writer, "{}", serde_json::to_string(&frame).unwrap()).unwrap();
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
            let frame = ServerFrame::Response {
                id: "1".to_owned(),
                envelope,
            };
            writeln!(writer, "{}", serde_json::to_string(&frame).unwrap()).unwrap();
        }
    });
}

/// The request inside a client frame: the object that names a `cmd`.
fn request(v: &Value) -> Option<&Value> {
    match v {
        Value::Object(m) if m.contains_key("cmd") => Some(v),
        Value::Object(m) => m.values().find_map(request),
        _ => None,
    }
}
