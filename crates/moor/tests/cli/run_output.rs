//! `moor run` through a controller prints the command's output as it arrives and exits with
//! the command's status.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use moor_core::controller::{write_bytes, ServerFrame, BYTES_MAX, PROTOCOL};
use moor_core::Response;
use serde_json::{json, Value};

use super::fresh_dir;

/// Exit status of the command every scripted run reports.
const EXIT: i32 = 3;

/// Serves one session on `socket`: answers the hello, takes one request, lets `script` write
/// frames for it, then answers with a run whose command exited with [`EXIT`]. Returns the
/// request.
fn serve_once(
    socket: &Path,
    script: impl FnOnce(&mut UnixStream, &str) + Send + 'static,
) -> JoinHandle<Value> {
    let listener = UnixListener::bind(socket).unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let hello = ServerFrame::Hello { protocol: PROTOCOL };
        stream.write_all(hello.to_line().as_bytes()).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        let id = frame["id"].as_str().unwrap().to_owned();
        script(&mut stream, &id);
        let value = json!({
            "ran": true, "run": 1, "exit": EXIT, "interrupted": false,
            "log": "/state/runs/1.log",
        });
        let envelope = Response::ok("run", Some(value));
        let answer = ServerFrame::Response { id, envelope };
        stream.write_all(answer.to_line().as_bytes()).unwrap();
        frame["request"].clone()
    })
}

/// The `moor` binary with a hermetic environment that routes through `socket`.
fn moor(socket: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_moor"));
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", fresh_dir("run-out-home"))
        .env("MOOR_CONTROLLER", socket)
        .args(args)
        .stdin(Stdio::null());
    cmd
}

/// A fresh directory and the controller socket path inside it.
fn socket() -> (PathBuf, PathBuf) {
    let root = fresh_dir("run-out");
    let socket = root.join("c.sock");
    (root, socket)
}

fn send_output(text: &'static [u8]) -> impl FnOnce(&mut UnixStream, &str) + Send + 'static {
    move |stream, id| write_bytes(stream, id, text).unwrap()
}

#[test]
fn run_output_goes_to_stdout_and_the_command_status_is_the_exit_code() {
    let (root, socket) = socket();
    let server = serve_once(&socket, send_output(b"built\nwarning: x\n"));
    let out = moor(&socket, &["run", "foo", "--", "make", "test"])
        .output()
        .unwrap();
    let request = server.join().unwrap();
    assert_eq!(request["cmd"], "run");
    assert_eq!(request["box"], "foo");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "built\nwarning: x\n");
    assert_eq!(out.status.code(), Some(EXIT));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn json_run_prints_output_to_stderr_and_one_envelope_to_stdout() {
    let (root, socket) = socket();
    let server = serve_once(&socket, send_output(b"built\n"));
    let out = moor(&socket, &["--json", "run", "foo", "--", "make"])
        .output()
        .unwrap();
    server.join().unwrap();
    let envelope: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {:?}", String::from_utf8_lossy(&out.stdout)));
    assert_eq!(envelope["class"], "ok");
    assert_eq!(envelope["value"]["exit"], EXIT);
    assert_eq!(String::from_utf8_lossy(&out.stderr), "built\n");
    assert_eq!(out.status.code(), Some(EXIT));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_interrupted_run_asks_the_controller_to_stop_and_exits_130() {
    let (root, socket) = socket();
    let listener = UnixListener::bind(&socket).unwrap();
    let (asked, request) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let hello = ServerFrame::Hello { protocol: PROTOCOL };
        stream.write_all(hello.to_line().as_bytes()).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let id = serde_json::from_str::<Value>(&line).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        asked.send(()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        line.clear();
        let stopped = matches!(reader.read_line(&mut line), Ok(0));
        let value = json!({
            "ran": true, "run": 1, "exit": null, "interrupted": stopped,
            "sync": "full", "log": "/state/runs/1.log",
        });
        let envelope = Response::ok("run", Some(value));
        let answer = ServerFrame::Response { id, envelope };
        stream.write_all(answer.to_line().as_bytes()).unwrap();
        stopped
    });
    let mut child = moor(&socket, &["run", "foo", "--", "sleep", "600"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    request
        .recv_timeout(Duration::from_secs(10))
        .expect("the run reached the controller");
    let pid = rustix::process::Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::INT).unwrap();
    let status = child.wait().unwrap();
    assert!(server.join().unwrap(), "the client shut its sending side");
    assert_eq!(status.code(), Some(130), "{status:?}");
    let _ = std::fs::remove_dir_all(root);
}

/// Output large enough that a client holding it would show in its peak memory.
const BIG: usize = 64 * 1024 * 1024;

/// Peak memory a client may reach while it streams [`BIG`] bytes: about 10 MiB above what the
/// client peaks at with a 64 KiB output, and half of `BIG`.
const PEAK_KB: u64 = 32 * 1024;

/// Byte `i` of the big output.
fn pattern(i: usize) -> u8 {
    u8::try_from(i % 251).unwrap()
}

/// Peak resident memory of process `pid`, in kB.
fn peak_kb(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let line = status.lines().find(|l| l.starts_with("VmHWM:")).unwrap();
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

/// What the controller saw before it answered: whether the client had printed every byte,
/// and the client's peak memory.
struct BeforeAnswer {
    printed_all: bool,
    peak_kb: u64,
}

#[test]
fn large_run_output_streams_with_bounded_client_memory() {
    let (root, socket) = socket();
    let (pid_tx, pid_rx) = mpsc::channel::<u32>();
    let (printed_tx, printed_rx) = mpsc::channel::<()>();
    let (seen_tx, seen_rx) = mpsc::channel::<BeforeAnswer>();
    let server = serve_once(&socket, move |stream, id| {
        let piece: Vec<u8> = (0..BYTES_MAX).map(pattern).collect();
        for _ in 0..BIG / BYTES_MAX {
            write_bytes(stream, id, &piece).unwrap();
        }
        let pid = pid_rx.recv().unwrap();
        let printed_all = printed_rx.recv_timeout(Duration::from_secs(20)).is_ok();
        seen_tx
            .send(BeforeAnswer {
                printed_all,
                peak_kb: peak_kb(pid),
            })
            .unwrap();
    });
    let mut child = moor(&socket, &["run", "foo", "--", "cat", "big"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    pid_tx.send(child.id()).unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut buf = vec![0; 1 << 16];
        let mut total = 0;
        let mut mismatch = None;
        loop {
            let n = stdout.read(&mut buf).unwrap();
            if n == 0 {
                return (total, mismatch);
            }
            for (k, b) in buf.iter().take(n).enumerate() {
                if mismatch.is_none() && *b != pattern((total + k) % BYTES_MAX) {
                    mismatch = Some(total + k);
                }
            }
            total += n;
            if total == BIG {
                // The controller stops waiting after a while; a late signal has no reader.
                printed_tx.send(()).ok();
            }
        }
    });
    let seen = seen_rx.recv_timeout(Duration::from_secs(60)).unwrap();
    assert!(
        seen.printed_all,
        "the client printed the output only at the end"
    );
    assert!(
        seen.peak_kb < PEAK_KB,
        "client peak memory {} kB while streaming {BIG} bytes",
        seen.peak_kb
    );
    assert_eq!(child.wait().unwrap().code(), Some(EXIT));
    server.join().unwrap();
    assert_eq!(reader.join().unwrap(), (BIG, None));
    let _ = std::fs::remove_dir_all(root);
}
