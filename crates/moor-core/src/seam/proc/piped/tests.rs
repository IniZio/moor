#![allow(clippy::panic, reason = "test assertions")]

use std::io::{Cursor, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::{pump, Piped, Pipes};

/// `sh -c script` with every stdio piped.
fn shell(script: &str) -> std::process::Child {
    Command::new("sh")
        .args(["-c", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn sh: {e}"))
}

/// Runs `script` with `input` on stdin; its stdout, stderr and end.
fn piped(script: &str, input: Vec<u8>) -> (Vec<u8>, Vec<u8>, Piped) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let ended = pump(
        shell(script),
        Vec::new(),
        Pipes {
            stdin: Box::new(Cursor::new(input)),
            stdout: &mut out,
            stderr: &mut err,
            stop: &|| false,
        },
    )
    .unwrap_or_else(|e| panic!("pump: {e}"));
    (out, err, ended)
}

/// A 1 MiB blob of every byte value, with the ssh escape sequences scattered through it.
fn blob() -> Vec<u8> {
    let mut data: Vec<u8> = (0..1024 * 1024u32)
        .map(|i| u8::try_from(i.wrapping_mul(2_654_435_761) >> 24).unwrap_or(0))
        .collect();
    for (at, seq) in [(0, &b"\n~."[..]), (4096, b"\n~C"), (500_000, b"\n~.\n~C~~")] {
        data.splice(at..at + seq.len(), seq.iter().copied());
    }
    data
}

#[test]
fn a_blob_with_escape_sequences_round_trips_through_cat_unchanged() {
    let data = blob();
    let (out, err, ended) = piped("cat", data.clone());
    assert!(out == data, "stdout differs from stdin");
    assert!(err.is_empty());
    assert_eq!(
        ended,
        Piped {
            code: Some(0),
            stopped: false
        }
    );
}

#[test]
fn stdout_and_stderr_come_back_apart_and_the_exit_code_propagates() {
    let (out, err, ended) = piped("echo out; echo err >&2; exit 7", Vec::new());
    assert_eq!(
        (out.as_slice(), err.as_slice()),
        (&b"out\n"[..], &b"err\n"[..])
    );
    assert_eq!(ended.code, Some(7));
}

#[test]
fn the_end_of_stdin_closes_the_command_stdin() {
    let (out, _, ended) = piped("wc -c; head -c1 >/dev/null; echo eof", b"abc".to_vec());
    assert_eq!(
        String::from_utf8_lossy(&out)
            .split_whitespace()
            .collect::<Vec<_>>(),
        ["3", "eof"]
    );
    assert_eq!(ended.code, Some(0));
}

#[test]
fn the_preamble_reaches_stdin_before_the_caller_bytes() {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let ended = pump(
        shell("cat"),
        b"env;".to_vec(),
        Pipes {
            stdin: Box::new(Cursor::new(b"data".to_vec())),
            stdout: &mut out,
            stderr: &mut err,
            stop: &|| false,
        },
    )
    .unwrap_or_else(|e| panic!("pump: {e}"));
    assert_eq!(out, b"env;data");
    assert_eq!(ended.code, Some(0));
}

/// A stdin that never ends, as a caller that keeps its side open.
struct Open;

impl Read for Open {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(Duration::from_secs(3600));
        Ok(0)
    }
}

#[test]
fn stop_kills_a_command_the_caller_left() {
    let left = AtomicBool::new(false);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let ended = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(200));
            left.store(true, Ordering::SeqCst);
        });
        pump(
            shell("exec sleep 30"),
            Vec::new(),
            Pipes {
                stdin: Box::new(Open),
                stdout: &mut out,
                stderr: &mut err,
                stop: &|| left.load(Ordering::SeqCst),
            },
        )
    })
    .unwrap_or_else(|e| panic!("pump: {e}"));
    assert_eq!(
        ended,
        Piped {
            code: None,
            stopped: true
        }
    );
}

/// A writer whose reader has gone.
struct Closed;

impl std::io::Write for Closed {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_caller_that_stops_taking_output_kills_the_command() {
    let mut err = Vec::new();
    let ended = pump(
        shell("echo hi; exec sleep 30"),
        Vec::new(),
        Pipes {
            stdin: Box::new(Open),
            stdout: &mut Closed,
            stderr: &mut err,
            stop: &|| false,
        },
    )
    .unwrap_or_else(|e| panic!("pump: {e}"));
    assert!(ended.stopped);
}
