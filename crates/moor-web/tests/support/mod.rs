//! A hermetic stack for end-to-end tests: a real `moor controller` with no
//! provider, and the `moor-web` daemon dialling it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    dead_code,
    unreachable_pub,
    reason = "test code: a failed setup step should abort the test"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

/// Distinguishes the token of each stack within one test binary.
static SEQ: AtomicU32 = AtomicU32::new(0);

/// Path of the `moor` binary, resolved once per test binary.
static MOOR: OnceLock<PathBuf> = OnceLock::new();

/// The `moor` binary next to `moor-web`, built once when it is missing.
///
/// `moor-web` must not depend on the `moor` crate, so cargo does not build
/// the binary for these tests.
fn moor_bin() -> &'static Path {
    MOOR.get_or_init(|| {
        let web = Path::new(env!("CARGO_BIN_EXE_moor-web"));
        let path = web.with_file_name("moor");
        if path.exists() {
            return path;
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let status = Command::new(env!("CARGO"))
            .args(["build", "-p", "moor", "--bin", "moor", "--locked"])
            .current_dir(root)
            .status()
            .expect("run cargo build for moor");
        assert!(status.success(), "cargo build -p moor failed");
        assert!(path.exists(), "moor binary not found at {}", path.display());
        path
    })
}

/// A controller and a web daemon in a private root directory.
pub struct Stack {
    /// Private root, removed on drop.
    root: tempfile::TempDir,
    /// The controller process.
    controller: Child,
    /// The web daemon process.
    web: Child,
    /// Controller socket path.
    socket: PathBuf,
    /// Port the web daemon listens on.
    port: u16,
    /// A `MOOR_FRONT_SOCKET` to set next to `--socket`, to show the flag wins.
    front_socket: Option<String>,
    /// Access token the daemon expects.
    token: String,
}

impl Stack {
    /// Starts the controller, then the web daemon, and waits for both.
    pub fn start() -> Self {
        Self::start_with("")
    }

    /// Like `start`, appending `extra` to the controller config.
    pub fn start_with(extra: &str) -> Self {
        // Kept short: Unix socket paths are limited to about 104 bytes.
        let root = tempfile::Builder::new().prefix("mw").tempdir().unwrap();
        let dir = root.path().to_path_buf();
        let socket = dir.join("c.sock");
        let config = dir.join("c.yaml");
        std::fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\n{extra}",
                socket.display(),
                dir.join("state").display()
            ),
        )
        .unwrap();
        let controller = Command::new(moor_bin())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", dir.join("home"))
            .env("MOOR_STATE_DIR", dir.join("state"))
            .args(["controller", "serve", "--config"])
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(dir.join("controller.err")).unwrap())
            .spawn()
            .unwrap();
        let mut stack = Self {
            root,
            controller,
            // Replaced below; a placeholder keeps Drop safe if startup panics.
            web: Command::new("true").spawn().unwrap(),
            socket,
            port: 0,
            front_socket: None,
            token: format!(
                "tok-{}-{}-{:x}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed),
                dir.as_os_str().len() * 7919
            ),
        };
        stack.wait_controller();
        stack.start_web(false);
        stack
    }

    /// Polls the controller socket for up to five seconds.
    fn wait_controller(&self) {
        for _ in 0..500 {
            if UnixStream::connect(&self.socket).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("controller never listened\n{}", self.diagnostics());
    }

    /// Stops the web daemon with SIGKILL and starts it again, leaving its socket to
    /// `MOOR_FRONT_SOCKET` when `via_env` is set.
    pub fn restart_web(&mut self, via_env: bool) {
        let _ = self.web.kill();
        let _ = self.web.wait();
        self.start_web(via_env);
    }

    /// Restarts the web daemon with `--socket` and a different `MOOR_FRONT_SOCKET`.
    pub fn restart_web_with_front_socket(&mut self, front: &str) {
        let _ = self.web.kill();
        let _ = self.web.wait();
        self.front_socket = Some(front.to_owned());
        self.start_web(false);
    }

    /// Sends SIGTERM to the web daemon's exact pid.
    pub fn terminate_web(&self) {
        let pid = i32::try_from(self.web.id()).unwrap();
        let pid = rustix::process::Pid::from_raw(pid).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    }

    /// The web daemon's exit status, waiting up to `limit` for it to exit.
    pub fn wait_web_exit(&mut self, limit: Duration) -> Option<std::process::ExitStatus> {
        let mut waited = Duration::ZERO;
        while waited < limit {
            if let Some(status) = self.web.try_wait().unwrap() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
            waited += Duration::from_millis(20);
        }
        None
    }

    /// A connection to the web daemon, for requests the test sends in pieces.
    pub fn connect_web(&self) -> TcpStream {
        let stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
    }

    /// The access token the daemon expects.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Spawns the web daemon and reads its URL line.
    fn start_web(&mut self, via_env: bool) {
        let dir = self.root.path().to_path_buf();
        let token_file = dir.join("token");
        std::fs::write(&token_file, &self.token).unwrap();
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut web = Command::new(env!("CARGO_BIN_EXE_moor-web"));
        web.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", dir.join("home"))
            .args(["--port", "0", "--token-file"])
            .arg(&token_file)
            .arg("--state-dir")
            .arg(dir.join("web"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(dir.join("web.err")).unwrap());
        if via_env {
            web.env("MOOR_FRONT_SOCKET", &self.socket);
        } else {
            web.arg("--socket").arg(&self.socket);
            if let Some(front) = &self.front_socket {
                web.env("MOOR_FRONT_SOCKET", front);
            }
        }
        let mut web = web.spawn().unwrap();
        let stdout = web.stdout.take().unwrap();
        self.web = web;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let line = rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("moor-web printed no URL\n{}", self.diagnostics()));
        let port = line
            .trim()
            .trim_end_matches('/')
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| panic!("bad URL line {line:?}\n{}", self.diagnostics()));
        self.port = port;
    }

    /// One HTTP/1.1 exchange; returns the status code and body.
    fn http(&self, method: &str, path: &str, extra: &str, body: &str) -> (u16, String) {
        let port = self.port;
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
             Authorization: Bearer {}\r\n{extra}\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.token,
            body.len()
        );
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("bad response head {head:?}\n{}", self.diagnostics()));
        (status, rest.to_owned())
    }

    /// Authenticated GET.
    pub fn get(&self, path: &str) -> (u16, String) {
        self.http("GET", path, "", "")
    }

    /// Authenticated GET with the header lines `extra`; passes each piece of the body to
    /// `on_body` with its offset as it arrives, and returns the response head.
    pub fn stream(&self, path: &str, extra: &str, mut on_body: impl FnMut(u64, &[u8])) -> String {
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\n{extra}\
             Connection: close\r\n\r\n",
            self.port, self.token
        );
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut reader = BufReader::with_capacity(64 * 1024, stream);
        let mut head = String::new();
        while reader.read_line(&mut head).unwrap() > 2 {}
        let mut buf = vec![0; 64 * 1024];
        let mut offset = 0;
        loop {
            let n = reader.read(&mut buf).unwrap();
            if n == 0 {
                return head;
            }
            on_body(offset, &buf[..n]);
            offset += n as u64;
        }
    }

    /// Process ids of the controller and the web daemon.
    pub fn pids(&self) -> (u32, u32) {
        (self.controller.id(), self.web.id())
    }

    /// The controller's state directory.
    pub fn state_dir(&self) -> PathBuf {
        self.root.path().join("state")
    }

    /// Authenticated JSON POST with the CSRF header and a same-origin `Origin`.
    pub fn post(&self, path: &str, body: &Value) -> (u16, String) {
        let extra = format!(
            "Content-Type: application/json\r\nX-Moor-Csrf: 1\r\n\
             Origin: http://127.0.0.1:{}\r\n",
            self.port
        );
        self.http("POST", path, &extra, &body.to_string())
    }

    /// One request at the current controller protocol straight to the controller; returns the envelope.
    pub fn controller(&self, request: impl std::borrow::Borrow<Value>) -> Value {
        let stream = UnixStream::connect(&self.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let hello = json!({"frame": "hello", "protocol": moor_core::controller::PROTOCOL});
        let req = json!({"frame": "request", "id": "1", "request": request.borrow()});
        writeln!(writer, "{hello}").unwrap();
        writeln!(writer, "{req}").unwrap();
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).unwrap();
            assert!(
                n > 0,
                "controller closed the socket\n{}",
                self.diagnostics()
            );
            let frame: Value = serde_json::from_str(&line).unwrap();
            match frame["frame"].as_str() {
                Some("response") => return frame["envelope"].clone(),
                Some("hello" | "progress") => {}
                _ => panic!("unexpected frame {frame}"),
            }
        }
    }

    /// Polls `GET /jobs/<job>` until it stops running, for up to 20 seconds.
    pub fn wait_job(&self, job: &str) -> Value {
        for _ in 0..200 {
            let (status, body) = self.get(&format!("/jobs/{job}"));
            assert_eq!(status, 200, "job lookup: {body}\n{}", self.diagnostics());
            let snapshot: Value = serde_json::from_str(&body).unwrap();
            if snapshot["state"] != "running" {
                return snapshot;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("job {job} still running\n{}", self.diagnostics());
    }

    /// Both daemons' stderr logs, for assertion messages.
    pub fn diagnostics(&self) -> String {
        let read =
            |name: &str| std::fs::read_to_string(self.root.path().join(name)).unwrap_or_default();
        format!(
            "--- controller stderr ---\n{}\n--- moor-web stderr ---\n{}",
            read("controller.err"),
            read("web.err")
        )
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        let _ = self.web.kill();
        let _ = self.web.wait();
        let _ = self.controller.kill();
        let _ = self.controller.wait();
    }
}
