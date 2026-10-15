//! End-to-end tests for `moor controller serve`: the socket must answer exactly as `moor call`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

use super::fresh_dir;

/// A hermetic environment shared by the controller and every `moor call`.
struct Sandbox {
    /// Root holding home, state and the socket.
    root: PathBuf,
    /// Controller config file.
    config: PathBuf,
    /// Controller socket path.
    socket: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = fresh_dir("ctl");
        // Kept short: Unix socket paths are limited to about 104 bytes.
        let socket = root.join("c.sock");
        let config = root.join("c.yaml");
        std::fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\n",
                socket.display(),
                root.join("state").display()
            ),
        )
        .unwrap();
        Self {
            root,
            config,
            socket,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_moor"));
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root.join("home"))
            .env("MOOR_STATE_DIR", self.root.join("state"));
        cmd
    }

    fn serve(&self) -> Command {
        let mut cmd = self.command();
        cmd.args(["controller", "serve", "--config"])
            .arg(&self.config);
        cmd
    }

    /// `moor call` stdout for `input`, parsed.
    fn call(&self, input: &str) -> Value {
        let mut child = self
            .command()
            .arg("call")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A running controller, killed on drop.
struct Controller {
    child: Child,
}

impl Controller {
    fn start(sandbox: &Sandbox) -> Self {
        let child = sandbox
            .serve()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let this = Self { child };
        // 500 polls of 10ms bound the wait at about five seconds.
        for _ in 0..500 {
            if UnixStream::connect(&sandbox.socket).is_ok() {
                return this;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("controller never listened");
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One client connection speaking newline-delimited JSON.
struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    fn connect(socket: &Path) -> Self {
        let writer = UnixStream::connect(socket).unwrap();
        writer
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        Self {
            reader: BufReader::new(writer.try_clone().unwrap()),
            writer,
        }
    }

    /// Sends one raw line and returns the next frame.
    fn send(&mut self, line: &str) -> Value {
        writeln!(self.writer, "{line}").unwrap();
        let mut reply = String::new();
        self.reader.read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).unwrap()
    }

    fn hello(&mut self) {
        let reply = self.send(r#"{"frame":"hello","protocol":1}"#);
        assert_eq!(reply, json!({"frame": "hello", "protocol": 1}));
    }

    fn request(&mut self, id: &str, request: &str) -> Value {
        let frame = format!(r#"{{"frame":"request","id":"{id}","request":{request}}}"#);
        let reply = self.send(&frame);
        assert_eq!(field(&reply, "frame"), "response", "{reply}");
        assert_eq!(field(&reply, "id"), id);
        field(&reply, "envelope").clone()
    }
}

#[test]
fn socket_envelope_equals_call_envelope() {
    let sandbox = Sandbox::new();
    let _controller = Controller::start(&sandbox);
    let mut client = Client::connect(&sandbox.socket);
    client.hello();
    let inputs = [
        r#"{"cmd":"version"}"#,
        r#"{"cmd":"show"}"#,
        r#"{"cmd":"show","task":"nope"}"#,
        r#"{"cmd":"start","name":"foo"}"#,
        r#"{"cmd":"run","task":"foo","argv":["true"]}"#,
    ];
    for (n, input) in inputs.iter().enumerate() {
        let over_socket = client.request(&n.to_string(), input);
        assert_eq!(over_socket, sandbox.call(input), "input {input}");
    }
}

#[test]
fn second_instance_names_the_first() {
    let sandbox = Sandbox::new();
    let first = Controller::start(&sandbox);
    let out = sandbox.serve().output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains(&first.child.id().to_string()), "{stderr}");
    assert!(
        stderr.contains(&sandbox.socket.display().to_string()),
        "{stderr}"
    );
}

#[test]
fn socket_is_owner_only() {
    let sandbox = Sandbox::new();
    let _controller = Controller::start(&sandbox);
    let mode = std::fs::metadata(&sandbox.socket)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn frame_errors_keep_the_connection_open() {
    let sandbox = Sandbox::new();
    let _controller = Controller::start(&sandbox);
    let mut client = Client::connect(&sandbox.socket);
    let bad = client.send(r#"{"frame":"hello","protocol":99}"#);
    assert_eq!(field(&bad, "frame"), "error");
    assert_eq!(field(&bad, "kind"), "unsupported-protocol");
    let garbage = client.send("not json");
    assert_eq!(field(&garbage, "frame"), "error");
    assert_eq!(field(&garbage, "kind"), "malformed");
    client.hello();
    let envelope = client.request("1", r#"{"cmd":"version"}"#);
    assert_eq!(envelope, sandbox.call(r#"{"cmd":"version"}"#));
}

/// A required member of a frame.
fn field<'a>(frame: &'a Value, key: &str) -> &'a Value {
    frame.get(key).unwrap()
}
