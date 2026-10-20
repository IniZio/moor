use std::io::{self, BufRead, Cursor, Read, Write};
use std::sync::mpsc;
use std::thread::{self, sleep};
use std::time::Duration;

use super::report_while;
use moor_core::controller::{parse_server_frame, ServerFrame};
use moor_core::Response;

const TICK: Duration = Duration::from_millis(40);

fn done() -> Response {
    Response::ok("start", None)
}

/// Each frame in `wire`, with the raw bytes that follow a bytes frame.
fn frames(wire: &[u8]) -> Vec<(ServerFrame, Vec<u8>)> {
    let mut reader = Cursor::new(wire);
    let mut out = Vec::new();
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap() > 0 {
        let frame = parse_server_frame(line.trim_end()).unwrap();
        let mut data = Vec::new();
        if let ServerFrame::Bytes { len, .. } = frame {
            data.resize(len, 0);
            reader.read_exact(&mut data).unwrap();
        }
        out.push((frame, data));
        line.clear();
    }
    out
}

/// A client that is gone: every write fails, and the attempts are counted.
struct Gone(usize);

impl Write for Gone {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        self.0 += 1;
        Err(io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn slow_work_emits_progress_with_the_request_id() {
    let mut wire = Vec::new();
    let response = report_while("start", "r7", TICK, &mut wire, |_| {
        sleep(TICK * 3 + TICK / 2);
        done()
    });
    assert_eq!(response.cmd, "start");
    let frames = frames(&wire);
    assert!(frames.len() >= 2, "got {} frames", frames.len());
    for (frame, _) in &frames {
        assert!(
            matches!(frame, ServerFrame::Progress { id, message }
                if id == "r7" && message.starts_with("start: still running (")),
            "{frame:?}"
        );
    }
}

#[test]
fn fast_work_emits_nothing() {
    let mut wire = Vec::new();
    let response = report_while("wait", "r1", Duration::from_secs(30), &mut wire, |_| done());
    assert_eq!(response.cmd, "start");
    assert!(wire.is_empty());
}

#[test]
fn output_goes_out_as_bytes_frames_in_order() {
    let mut wire = Vec::new();
    report_while("run", "r3", Duration::from_secs(30), &mut wire, |sink| {
        sink(b"one\n").unwrap();
        sink(b"two\0\xff").unwrap();
        done()
    });
    let bytes = |len| ServerFrame::Bytes {
        id: "r3".to_owned(),
        len,
    };
    assert_eq!(
        frames(&wire),
        [
            (bytes(4), b"one\n".to_vec()),
            (bytes(5), b"two\0\xff".to_vec())
        ]
    );
}

#[test]
fn a_gone_client_stops_emits_but_work_finishes() {
    let mut gone = Gone(0);
    let response = report_while("run", "r2", TICK, &mut gone, |_| {
        sleep(TICK * 4 + TICK / 2);
        done()
    });
    assert_eq!(response.cmd, "start");
    assert_eq!(gone.0, 1);
}

#[test]
fn a_gone_client_never_blocks_output() {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let response = report_while("run", "r4", TICK, &mut Gone(0), |sink| {
            for _ in 0..100 {
                let _ = sink(b"piece");
            }
            done()
        });
        tx.send(response).ok();
    });
    let response = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("work blocked on output nobody reads");
    assert_eq!(response.cmd, "start");
}
