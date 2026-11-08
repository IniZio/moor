//! `moor-hc exec` through a fake controller socket: every byte value goes in and comes back
//! unchanged, stderr stays apart, the command's exit code is moor-hc's, and a Box that is not
//! up is refused before any exec reaches the controller.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "the fake controller panics its thread, which the test then sees"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use moor_core::controller::{
    parse_frame, write_bytes, write_stderr, ClientFrame, ServerFrame, PROTOCOL,
};
use moor_core::seam::proc::{pump, Pipes};
use moor_core::view::ExecView;
use moor_core::{ExecReq, Request, Response};
use moor_testkit::ChildGuard;
use serde_json::json;

/// The client's stdin frames as a reader: bytes frames give their bytes, an eof frame ends it.
struct Frames {
    /// The connection after the request line.
    conn: BufReader<UnixStream>,
    /// Bytes of the last frame the caller has not read yet.
    held: Vec<u8>,
}

impl Read for Frames {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.held.is_empty() {
            let mut line = String::new();
            if self.conn.read_line(&mut line)? == 0 {
                return Ok(0);
            }
            let Ok(ClientFrame::Bytes { len, .. }) = parse_frame(line.trim_end()) else {
                return Ok(0);
            };
            self.held.resize(len, 0);
            self.conn.read_exact(&mut self.held)?;
        }
        let n = self.held.as_slice().read(buf)?;
        self.held.drain(..n);
        Ok(n)
    }
}

/// One output stream of the command, written as frames by `write`.
struct Out<'a>(
    &'a Mutex<UnixStream>,
    fn(&mut dyn Write, &str, &[u8]) -> std::io::Result<()>,
);

impl Write for Out<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut conn = self.0.lock().unwrap();
        (self.1)(&mut *conn, "1", buf).map(|()| buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Runs `req` on this computer, one word as a shell line, as the controller runs it in the Box.
fn run_here(req: &ExecReq, conn: UnixStream, reader: BufReader<UnixStream>) -> Response {
    let (program, args) = match req.argv.as_slice() {
        [shell] => ("sh", vec!["-c".to_owned(), shell.clone()]),
        [program, args @ ..] => (program.as_str(), args.to_vec()),
        [] => panic!("no command"),
    };
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let shared = Mutex::new(conn);
    let (mut out, mut err) = (Out(&shared, write_bytes), Out(&shared, write_stderr));
    let pipes = Pipes {
        stdin: Box::new(Frames {
            conn: reader,
            held: Vec::new(),
        }),
        stdout: &mut out,
        stderr: &mut err,
        stop: &|| false,
    };
    let ended = pump(child, Vec::new(), pipes).unwrap();
    let view = ExecView {
        exit: ended.code,
        timed_out: false,
    };
    Response::ok("exec", Some(serde_json::to_value(view).unwrap()))
}

/// Answers `show` with one Box in `lifecycle`, and runs an `exec` on this computer.
fn answer(conn: UnixStream, lifecycle: &str, seen: &Mutex<Vec<String>>) {
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let mut writer = conn.try_clone().unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let hello = ServerFrame::Hello { protocol: PROTOCOL };
    writer.write_all(hello.to_line().as_bytes()).unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    let Ok(ClientFrame::Request { request, .. }) = parse_frame(line.trim_end()) else {
        panic!("expected a request, got {line}");
    };
    seen.lock().unwrap().push(request.cmd().to_owned());
    let envelope = match *request {
        Request::Show(req) => {
            let row =
                json!({"name": "moor/a", "lifecycle": lifecycle, "agent": {"launched": true}});
            let value = if req.target.is_some() {
                json!({"box": row})
            } else {
                json!({"boxes": [row]})
            };
            Response::ok("show", Some(value))
        }
        Request::Exec(req) => run_here(&req, conn, reader),
        other => panic!("unexpected request {other:?}"),
    };
    let frame = ServerFrame::Response {
        id: "1".to_owned(),
        envelope,
    };
    writer.write_all(frame.to_line().as_bytes()).unwrap();
}

/// A fake controller on `socket` whose Box is in `lifecycle`; returns the commands it saw.
fn serve(socket: &Path, lifecycle: &'static str) -> Arc<Mutex<Vec<String>>> {
    let listener = UnixListener::bind(socket).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    thread::spawn(move || {
        for conn in listener.incoming() {
            answer(conn.unwrap(), lifecycle, &log);
        }
    });
    seen
}

/// Runs `moor-hc exec moor/a -- <shell>` against a fake controller with `stdin`.
fn exec(lifecycle: &'static str, shell: &str, stdin: Vec<u8>) -> (Output, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("c.sock");
    let seen = serve(&socket, lifecycle);
    let child = Command::new(env!("CARGO_BIN_EXE_moor-hc"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir.path())
        .args([
            "--socket",
            &socket.to_string_lossy(),
            "exec",
            "moor/a",
            "--",
        ])
        .arg(shell)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = ChildGuard::new(child);
    let mut input = child.stdin.take().unwrap();
    let feeder = thread::spawn(move || input.write_all(&stdin));
    let out = child.wait_with_output().unwrap();
    feeder.join().unwrap().unwrap();
    let seen = seen.lock().unwrap().clone();
    (out, seen)
}

#[test]
fn every_byte_goes_through_unchanged_with_stderr_apart_and_the_exit_code_kept() {
    let input: Vec<u8> = (0..=255u8).cycle().take(1 << 20).collect();
    let (out, seen) = exec(
        "ready",
        "cat; printf 'to stderr' >&2; exit 7",
        input.clone(),
    );
    assert_eq!(out.stdout.len(), input.len());
    assert!(out.stdout == input, "stdout differs from stdin");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "to stderr");
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(seen, ["show", "exec"]);
}

#[test]
fn an_asleep_box_is_refused_and_never_woken() {
    let (out, seen) = exec("asleep", "true", Vec::new());
    assert_eq!(out.status.code(), Some(255));
    assert!(out.stdout.is_empty());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("moor/a is not up"), "{said}");
    assert_eq!(seen, ["show"]);
}
