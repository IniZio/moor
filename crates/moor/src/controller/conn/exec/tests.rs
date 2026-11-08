//! `exec` through the controller socket, between the real client and the fake driver, which
//! runs the command on the host.

use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use moor_core::controller::{ClientFrame, PROTOCOL};
use moor_core::view::ExecView;
use moor_core::{BoxName, ExecReq, Request, Response};

use super::super::super::drain::Admitted;
use super::super::serve;
use super::{answer, stream, Link};
use crate::client::{exec, ExecIo, Target};
use crate::env::Env;
use crate::ops::run_fixture::{ready_with, Fx};
use crate::testing::driver::FakeDriver;
use crate::Moor;

/// Serves one connection on a fresh socket, answering an `exec` with the fixture's engine.
/// The handle ends once the connection is served.
fn controller(f: Fx) -> (PathBuf, JoinHandle<()>, tempfile::TempDir) {
    let sock = f.dir.path().join("c.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let cx = Mutex::new(Some(f.cx));
    let served = thread::spawn(move || {
        let (conn, _) = listener.accept().unwrap();
        let reader = BufReader::new(conn.try_clone().unwrap());
        let waker = conn.try_clone().unwrap();
        serve(
            reader,
            conn,
            &Admitted::unwatched(),
            |id, request, (input, output)| match request {
                Request::Exec(req) => {
                    let moor = Moor::from_cx(cx.lock().unwrap().take().unwrap());
                    let link = Link {
                        id,
                        input,
                        output,
                        unblock: &|| drop(waker.shutdown(Shutdown::Read)),
                        stopping: &|| false,
                    };
                    Some(answer("exec", stream(link, |pipes| moor.exec(&req, pipes))))
                }
                other => Some(Response::ok(other.cmd(), None)),
            },
            |_, _, _, _, _| {},
        );
    });
    (sock, served, f.dir)
}

/// `exec` of `argv` in Box `t`.
fn req(argv: &[&str]) -> ExecReq {
    ExecReq {
        target: BoxName::parse_allow_bare("t").unwrap().into(),
        argv: argv.iter().map(|s| (*s).to_owned()).collect(),
        timeout_secs: None,
    }
}

/// Runs `argv` through the controller with `stdin`; its stdout, stderr and exit code.
fn run(argv: &[&str], stdin: Option<Vec<u8>>) -> (Vec<u8>, Vec<u8>, Option<i32>) {
    let (sock, served, _dir) = controller(ready_with(FakeDriver::new()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let io = ExecIo {
        stdin: stdin.map(|b| Box::new(Cursor::new(b)) as Box<dyn Read + Send>),
        stdout: &mut out,
        stderr: &mut err,
    };
    let resp = exec(&Env::default(), &Target::Socket(sock), req(argv), io).unwrap();
    assert!(resp.error.is_none(), "{resp:?}");
    let view: ExecView = serde_json::from_value(resp.value.unwrap()).unwrap();
    served.join().unwrap();
    (out, err, view.exit)
}

/// A 1 MiB blob of every byte value, with the ssh escape sequences scattered through it.
fn blob() -> Vec<u8> {
    let mut data: Vec<u8> = (0..1024 * 1024u32)
        .map(|i| u8::try_from(i.wrapping_mul(2_654_435_761) >> 24).unwrap_or(0))
        .collect();
    for (at, seq) in [(0, &b"\n~."[..]), (65_530, b"\n~C"), (700_001, b"\n~.\n~C")] {
        data.splice(at..at + seq.len(), seq.iter().copied());
    }
    data
}

#[test]
fn a_blob_with_escape_sequences_round_trips_through_cat_unchanged() {
    let data = blob();
    let (out, err, exit) = run(&["cat"], Some(data.clone()));
    assert!(out == data, "stdout differs from stdin");
    assert!(err.is_empty());
    assert_eq!(exit, Some(0));
}

#[test]
fn stdout_and_stderr_come_back_apart_in_order_and_the_exit_code_propagates() {
    let script = "echo one; echo two >&2; echo three; echo four >&2; exit 7";
    let (out, err, exit) = run(&[script], None);
    assert_eq!(String::from_utf8_lossy(&out), "one\nthree\n");
    assert_eq!(String::from_utf8_lossy(&err), "two\nfour\n");
    assert_eq!(exit, Some(7));
}

#[test]
fn stdin_bytes_reach_the_command_and_its_end_closes_the_command_stdin() {
    let (out, _, exit) = run(&["head", "-n1"], Some(b"hi\nthere\n".to_vec()));
    assert_eq!(out, b"hi\n");
    assert_eq!(exit, Some(0));
    let (out, _, _) = run(&["wc -c; echo after"], Some(b"abc".to_vec()));
    assert_eq!(
        String::from_utf8_lossy(&out)
            .split_whitespace()
            .collect::<Vec<_>>(),
        ["3", "after"]
    );
    let (out, _, _) = run(&["wc -c"], None);
    assert_eq!(String::from_utf8_lossy(&out).trim(), "0");
}

#[test]
fn a_client_that_goes_away_kills_the_command() {
    let (sock, served, _dir) = controller(ready_with(FakeDriver::new()));
    let conn = UnixStream::connect(&sock).unwrap();
    let mut writer = conn.try_clone().unwrap();
    let mut reader = BufReader::new(conn);
    writer
        .write_all(
            ClientFrame::Hello { protocol: PROTOCOL }
                .to_line()
                .as_bytes(),
        )
        .unwrap();
    let request = ClientFrame::Request {
        id: "1".to_owned(),
        request: Box::new(Request::Exec(req(&["echo started; exec sleep 30"]))),
    };
    writer.write_all(request.to_line().as_bytes()).unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains("\"bytes\""), "expected output, got {line}");
    drop((reader, writer));
    let (done, ended) = std::sync::mpsc::channel();
    thread::spawn(move || done.send(served.join().is_ok()).unwrap());
    assert_eq!(ended.recv_timeout(Duration::from_secs(10)), Ok(true));
}
