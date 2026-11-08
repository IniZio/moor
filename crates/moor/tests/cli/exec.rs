//! `moor exec` through a controller: stdin reaches the command, stdout and stderr come back
//! apart, and the command's exit code is moor's. The controller here runs the command on the
//! host.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};

use moor_core::controller::{
    parse_frame, write_bytes, write_stderr, ClientFrame, ServerFrame, PROTOCOL,
};
use moor_core::seam::proc::{pump, Pipes};
use moor_core::view::ExecView;
use moor_core::{Request, Response};
use moor_testkit::ChildGuard;

use super::fresh_dir;

/// The client's stdin frames as a reader: bytes frames give their bytes, an eof frame ends it.
struct Frames(BufReader<UnixStream>);

impl Read for Frames {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut line = String::new();
        if self.0.read_line(&mut line)? == 0 {
            return Ok(0);
        }
        match parse_frame(line.trim_end()) {
            Ok(ClientFrame::Bytes { len, .. }) => {
                let part = buf.get_mut(..len).expect("a frame that fits the buffer");
                self.0.read_exact(part)?;
                Ok(len)
            }
            _ => Ok(0),
        }
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

/// Serves one `exec` on `socket` by running its argv on the host; returns the request.
fn serve_once(socket: &Path) -> JoinHandle<Request> {
    let listener = UnixListener::bind(socket).unwrap();
    thread::spawn(move || {
        let (conn, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let mut writer = conn.try_clone().unwrap();
        writer
            .write_all(
                ServerFrame::Hello { protocol: PROTOCOL }
                    .to_line()
                    .as_bytes(),
            )
            .unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let Ok(ClientFrame::Request { request, .. }) = parse_frame(line.trim_end()) else {
            panic!("expected a request, got {line}");
        };
        let Request::Exec(req) = (*request).clone() else {
            panic!("expected exec, got {request:?}");
        };
        let (program, args) = req.argv.split_first().expect("a command");
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
            stdin: Box::new(Frames(reader)),
            stdout: &mut out,
            stderr: &mut err,
            stop: &|| false,
        };
        let ended = pump(child, Vec::new(), pipes).unwrap();
        let view = ExecView {
            exit: ended.code,
            timed_out: false,
        };
        let envelope = Response::ok("exec", Some(serde_json::to_value(view).unwrap()));
        let answer = ServerFrame::Response {
            id: "1".to_owned(),
            envelope,
        };
        writer.write_all(answer.to_line().as_bytes()).unwrap();
        *request
    })
}

/// Runs `moor exec <args>` through a fresh controller with `stdin`.
fn exec(args: &[&str], stdin: &[u8]) -> (Output, Request) {
    let root = fresh_dir("exec");
    let socket: PathBuf = root.0.path().join("c.sock");
    let server = serve_once(&socket);
    let child = Command::new(env!("CARGO_BIN_EXE_moor"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.0.path())
        .env("MOOR_CONTROLLER", &socket)
        .arg("exec")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = ChildGuard::new(child);
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = child.wait_with_output().unwrap();
    (out, server.join().unwrap())
}

#[test]
fn piped_stdin_reaches_the_command_and_its_output_comes_back() {
    let (out, request) = exec(&["b", "--", "head", "-n1"], b"hi\nthere\n");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hi\n");
    assert_eq!(out.status.code(), Some(0));
    let sent = serde_json::to_value(&request).unwrap();
    assert_eq!(
        sent,
        serde_json::json!({"cmd": "exec", "box": "b", "argv": ["head", "-n1"]})
    );
}

#[test]
fn the_command_exit_code_is_moor_s() {
    let (out, _) = exec(&["b", "--", "sh", "-c", "exit 7"], b"");
    assert_eq!(out.status.code(), Some(7));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn stdout_and_stderr_stay_apart() {
    let (out, _) = exec(&["b", "--", "sh", "-c", "echo out; echo err >&2"], b"");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "out\n");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "err\n");
}
