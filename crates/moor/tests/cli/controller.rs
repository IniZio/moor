//! End-to-end tests for `moor controller serve`: the socket must answer exactly as `moor call`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use moor_core::controller::PROTOCOL;
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
        // Outside any git checkout, so `start` never fetches a live origin.
        cmd.current_dir(&self.root)
            .env_clear()
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
        self.next_frame()
    }

    fn next_frame(&mut self) -> Value {
        let mut reply = String::new();
        self.reader.read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).unwrap()
    }

    fn hello(&mut self) {
        let reply = self.send(&format!(r#"{{"frame":"hello","protocol":{PROTOCOL}}}"#));
        assert_eq!(reply, json!({"frame": "hello", "protocol": PROTOCOL}));
    }

    /// Sends one request and returns its envelope, skipping the progress frames
    /// a long verb may send first.
    fn request(&mut self, id: &str, request: &str) -> Value {
        let frame = format!(r#"{{"frame":"request","id":"{id}","request":{request}}}"#);
        let mut reply = self.send(&frame);
        // One progress frame per five seconds; 12 bound the wait at about a minute.
        for _ in 0..12 {
            if field(&reply, "frame") != "progress" {
                break;
            }
            assert_eq!(field(&reply, "id"), id, "{reply}");
            reply = self.next_frame();
        }
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
        r#"{"cmd":"show","box":"nope"}"#,
        r#"{"cmd":"start","name":"foo"}"#,
        r#"{"cmd":"run","box":"foo","argv":["true"]}"#,
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

/// Exit code of `child`, or `None` after killing it when it runs past five seconds.
fn exit_within_five_seconds(child: &mut Child) -> Option<i32> {
    for _ in 0..500 {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

#[test]
fn second_controller_on_another_socket_refuses_to_start() {
    let sandbox = Sandbox::new();
    let first = Controller::start(&sandbox);
    let config = sandbox.root.join("d.yaml");
    std::fs::write(
        &config,
        format!(
            "socket: {}\nstate_dir: {}\n",
            sandbox.root.join("d.sock").display(),
            sandbox.root.join("state").display()
        ),
    )
    .unwrap();
    let mut second = sandbox
        .command()
        .args(["controller", "serve", "--config"])
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let code = exit_within_five_seconds(&mut second);
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut second.stderr.take().unwrap(), &mut stderr).unwrap();
    assert_eq!(
        code,
        Some(1),
        "second controller on the same state dir: {stderr}"
    );
    assert!(stderr.contains(&first.child.id().to_string()), "{stderr}");
    assert!(
        stderr.contains(&sandbox.socket.display().to_string()),
        "{stderr}"
    );
}

/// The `schema_version` recorded in `db`, or `None` while it cannot be read.
fn schema_version(db: &Path) -> Option<String> {
    rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .ok()?
        .query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |r| {
            r.get(0)
        })
        .ok()
}

#[test]
fn upgrade_guard_sees_a_controller_on_a_custom_socket() {
    let sandbox = Sandbox::new();
    let mut config = std::fs::read_to_string(&sandbox.config).unwrap();
    config.push_str("settle_every_secs: 3600\n");
    std::fs::write(&sandbox.config, config).unwrap();
    let controller = Controller::start(&sandbox);
    let state = sandbox.root.join("state");
    let db = state.join("moor.db");
    Client::connect(&sandbox.socket).hello();
    let old = state.join("old.db");
    rusqlite::Connection::open(&old)
        .unwrap()
        .execute_batch(
            "CREATE TABLE meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
             INSERT INTO meta (k, v) VALUES ('schema_version', '1');",
        )
        .unwrap();
    std::fs::rename(&old, &db).unwrap();
    let out = sandbox.command().arg("list").output().unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_ne!(
        out.status.code(),
        Some(0),
        "old store retired under a running controller"
    );
    assert!(
        stderr.contains(&format!("controller (pid {})", controller.child.id())),
        "{stderr}"
    );
    assert_eq!(schema_version(&db).as_deref(), Some("1"));
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
