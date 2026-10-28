#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "tests assert on fixed fixtures"
)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::thread::JoinHandle;
use std::time::Duration;

use moor_core::request::ShowReq;
use moor_core::user::Identity;
use moor_core::{BoxId, BoxName, SessionId};

use super::*;

fn show() -> Request {
    Request::Show(ShowReq {
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    })
}

fn ok_envelope() -> String {
    serde_json::to_string(&Response::ok("show", None)).unwrap()
}

/// Accepts one connection; for each step reads one client line, then writes the step's
/// lines. Returns the client lines seen, after closing the connection.
fn serve(socket: &Path, steps: Vec<Vec<String>>) -> JoinHandle<Vec<String>> {
    let listener = UnixListener::bind(socket).unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut seen = Vec::new();
        for step in steps {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            seen.push(line.trim_end().to_owned());
            for out in step {
                writeln!(writer, "{out}").unwrap();
            }
        }
        seen
    })
}

fn hello() -> String {
    format!(r#"{{"frame":"hello","protocol":{PROTOCOL}}}"#)
}

fn error_frame() -> String {
    r#"{"frame":"error","kind":"slow-client","message":"too slow"}"#.to_owned()
}

fn event(cursor: i64, extra: &str) -> String {
    format!(
        r#"{{"frame":"event","cursor":{cursor},{extra}"kind":"box_ready","payload":{{"n":{cursor}}},"ts":{}}}"#,
        cursor * 10
    )
}

fn collect(watch: Watch) -> Vec<Result<WatchItem, ClientError>> {
    watch.collect()
}

#[test]
fn request_returns_envelope_and_progress_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(
        &sock,
        vec![
            vec![hello()],
            vec![
                r#"{"frame":"progress","id":"1","message":"one"}"#.to_owned(),
                r#"{"frame":"progress","id":"1","message":"two"}"#.to_owned(),
                format!(
                    r#"{{"frame":"response","id":"1","envelope":{}}}"#,
                    ok_envelope()
                ),
            ],
        ],
    );
    let mut notes = Vec::new();
    let resp = Client::new(&sock)
        .request(show(), |m| notes.push(m.to_owned()))
        .unwrap();
    assert_eq!(resp, Response::ok("show", None));
    assert_eq!(notes, ["one", "two"]);
    let seen = server.join().unwrap();
    assert_eq!(
        seen[0],
        format!(r#"{{"frame":"hello","protocol":{PROTOCOL}}}"#)
    );
    assert!(
        seen[1].starts_with(r#"{"frame":"request","id":"1""#),
        "{}",
        seen[1]
    );
}

#[test]
fn hello_with_wrong_protocol_is_a_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(
        &sock,
        vec![vec![r#"{"frame":"hello","protocol":99}"#.to_owned()]],
    );
    let err = Client::new(&sock).request(show(), |_| {}).unwrap_err();
    assert!(
        matches!(&err, ClientError::Protocol(m) if m.contains("99")),
        "{err}"
    );
    server.join().unwrap();
}

#[test]
fn error_frame_surfaces_with_its_kind() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(&sock, vec![vec![hello()], vec![error_frame()]]);
    let err = Client::new(&sock).request(show(), |_| {}).unwrap_err();
    assert!(
        matches!(&err, ClientError::Frame { kind: FrameErrorKind::SlowClient, message } if message == "too slow"),
        "{err}"
    );
    server.join().unwrap();
}

#[test]
fn eof_before_response_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(
        &sock,
        vec![
            vec![hello()],
            vec![r#"{"frame":"progress","id":"1","message":"one"}"#.to_owned()],
        ],
    );
    let err = Client::new(&sock).request(show(), |_| {}).unwrap_err();
    assert!(matches!(err, ClientError::Protocol(_)), "{err}");
    server.join().unwrap();
}

#[test]
fn malformed_frame_is_a_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(&sock, vec![vec![hello()], vec!["not json".to_owned()]]);
    let err = Client::new(&sock).request(show(), |_| {}).unwrap_err();
    assert!(matches!(err, ClientError::Protocol(_)), "{err}");
    server.join().unwrap();
}

#[test]
fn missing_socket_is_a_connect_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = Client::new(dir.path().join("none.sock"))
        .with_redial(Redial::NEVER)
        .request(show(), |_| {})
        .unwrap_err();
    assert!(matches!(err, ClientError::Connect { .. }), "{err}");
}

#[test]
fn watch_sends_since_filter_and_follow() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(&sock, vec![vec![hello()], vec![]]);
    let filter = WatchFilter {
        boxes: Some(vec![BoxName::parse("repo/one").unwrap()]),
        kinds: Some(vec!["box_ready".to_owned()]),
        parent: Some(moor_core::SessionId::new(9)),
    };
    let watch = Client::new(&sock).watch(42, filter, false).unwrap();
    assert!(collect(watch).is_empty());
    let seen = server.join().unwrap();
    let sent: serde_json::Value = serde_json::from_str(&seen[1]).unwrap();
    assert_eq!(
        sent,
        serde_json::json!({
            "frame": "watch",
            "since": 42,
            "boxes": ["repo/one"],
            "kinds": ["box_ready"],
            "parent": 9,
            "follow": false,
        })
    );
}

#[test]
fn watch_follow_omits_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(&sock, vec![vec![hello()], vec![]]);
    let watch = Client::new(&sock)
        .watch(0, WatchFilter::default(), true)
        .unwrap();
    assert!(collect(watch).is_empty());
    let seen = server.join().unwrap();
    let sent: serde_json::Value = serde_json::from_str(&seen[1]).unwrap();
    assert_eq!(sent, serde_json::json!({"frame": "watch", "since": 0}));
}

#[test]
fn an_acting_client_asserts_its_identity_on_requests_and_watches() {
    let identity = Identity::parse("slack:T1/U1").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let answer = format!(
        r#"{{"frame":"response","id":"1","envelope":{}}}"#,
        ok_envelope()
    );
    let server = serve(&sock, vec![vec![hello()], vec![answer]]);
    let acting = Client::new(&sock).acting_as(identity);
    acting.request(show(), |_| {}).unwrap();
    let seen = server.join().unwrap();
    let sent: serde_json::Value = serde_json::from_str(&seen[1]).unwrap();
    assert_eq!(sent["request"]["cmd"], "as");
    assert_eq!(sent["request"]["identity"], "slack:T1/U1");
    assert_eq!(sent["request"]["request"]["cmd"], "show");
    std::fs::remove_file(&sock).unwrap();
    let server = serve(&sock, vec![vec![hello()], vec![]]);
    let watch = acting.watch(0, WatchFilter::default(), true).unwrap();
    assert!(collect(watch).is_empty());
    let seen = server.join().unwrap();
    let sent: serde_json::Value = serde_json::from_str(&seen[1]).unwrap();
    assert_eq!(
        sent,
        serde_json::json!({"frame": "watch", "since": 0, "identity": "slack:T1/U1"})
    );
}

#[test]
fn watch_yields_events_then_caught_up_then_ends() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(
        &sock,
        vec![
            vec![hello()],
            vec![
                event(1, r#""box":"repo/one","box_id":4,"session":7,"#),
                event(2, ""),
                r#"{"frame":"caught-up","cursor":2}"#.to_owned(),
            ],
        ],
    );
    let watch = Client::new(&sock)
        .watch(0, WatchFilter::default(), false)
        .unwrap();
    let items: Vec<WatchItem> = collect(watch).into_iter().map(Result::unwrap).collect();
    assert_eq!(
        items,
        vec![
            WatchItem::Event(EventRecord {
                cursor: 1,
                box_name: Some("repo/one".to_owned()),
                box_id: Some(BoxId::new(4)),
                session: Some(SessionId::new(7)),
                kind: "box_ready".to_owned(),
                payload: serde_json::json!({"n": 1}),
                ts: 10,
            }),
            WatchItem::Event(EventRecord {
                cursor: 2,
                box_name: None,
                box_id: None,
                session: None,
                kind: "box_ready".to_owned(),
                payload: serde_json::json!({"n": 2}),
                ts: 20,
            }),
            WatchItem::CaughtUp { cursor: 2 },
        ]
    );
    server.join().unwrap();
}

#[test]
fn watch_error_frame_is_surfaced_then_ends() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = serve(
        &sock,
        vec![vec![hello()], vec![event(1, ""), error_frame()]],
    );
    let mut watch = Client::new(&sock)
        .watch(0, WatchFilter::default(), true)
        .unwrap();
    assert!(matches!(watch.next(), Some(Ok(WatchItem::Event(_)))));
    let err = watch.next().unwrap().unwrap_err();
    assert!(
        matches!(
            err,
            ClientError::Frame {
                kind: FrameErrorKind::SlowClient,
                ..
            }
        ),
        "{err}"
    );
    assert!(watch.next().is_none());
    server.join().unwrap();
}

/// Accepts one connection, answers hello, then stays silent until the client hangs up.
fn quiet(socket: &Path) -> JoinHandle<()> {
    let listener = UnixListener::bind(socket).unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        writeln!(writer, "{}", hello()).unwrap();
        while reader.read_line(&mut line).unwrap() > 0 {}
    })
}

fn fast(socket: &Path) -> Client {
    Client::new(socket).with_timeouts(Timeouts {
        read: Duration::from_millis(200),
        write: Duration::from_secs(5),
    })
}

#[test]
fn silent_controller_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = quiet(&sock);
    let err = fast(&sock).request(show(), |_| {}).unwrap_err();
    assert!(matches!(err, ClientError::Timeout), "{err}");
    server.join().unwrap();
}

#[test]
fn read_timeout_is_per_frame() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        writeln!(writer, "{}", hello()).unwrap();
        reader.read_line(&mut line).unwrap();
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(100));
            writeln!(writer, r#"{{"frame":"progress","id":"1","message":"x"}}"#).unwrap();
        }
        writeln!(
            writer,
            r#"{{"frame":"response","id":"1","envelope":{}}}"#,
            ok_envelope()
        )
        .unwrap();
    });
    let mut notes = 0;
    let resp = fast(&sock).request(show(), |_| notes += 1).unwrap();
    assert_eq!(resp, Response::ok("show", None));
    assert_eq!(notes, 4);
    server.join().unwrap();
}

#[test]
fn oversize_frame_is_a_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let big = "a".repeat(MAX_FRAME + 10);
    let server = serve(&sock, vec![vec![hello()], vec![big]]);
    let err = Client::new(&sock).request(show(), |_| {}).unwrap_err();
    assert!(
        matches!(&err, ClientError::Protocol(m) if m.contains("longer than")),
        "{err}"
    );
    drop(server);
}

#[test]
fn stopper_unblocks_a_quiet_watch() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let server = quiet(&sock);
    let mut watch = fast(&sock).watch(0, WatchFilter::default(), true).unwrap();
    let stop = watch.stopper();
    let closer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        stop.stop();
    });
    assert!(watch.next().is_none());
    closer.join().unwrap();
    server.join().unwrap();
}

/// Accepts one connection, answers hello, reads the request, then writes `reply` raw and
/// hangs up.
fn answer_raw(socket: &Path, reply: Vec<u8>) -> JoinHandle<()> {
    let listener = UnixListener::bind(socket).unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        writeln!(writer, "{}", hello()).unwrap();
        reader.read_line(&mut line).unwrap();
        let _ = writer.write_all(&reply);
    })
}

fn bytes_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = format!(r#"{{"frame":"bytes","id":"1","len":{}}}"#, payload.len()).into_bytes();
    frame.push(b'\n');
    frame.extend_from_slice(payload);
    frame
}

fn response_line() -> Vec<u8> {
    format!(
        "{{\"frame\":\"response\",\"id\":\"1\",\"envelope\":{}}}\n",
        ok_envelope()
    )
    .into_bytes()
}

#[test]
fn read_passes_each_bytes_frame_in_order_then_returns_the_response() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let reply = [bytes_frame(b"ab"), bytes_frame(b"\n\xff"), response_line()].concat();
    let server = answer_raw(&sock, reply);
    let mut got = Vec::new();
    let resp = Client::new(&sock)
        .read(show(), |data| {
            got.push(data.to_vec());
            Ok(())
        })
        .unwrap();
    assert_eq!(resp, Response::ok("show", None));
    assert_eq!(got, [b"ab".to_vec(), b"\n\xff".to_vec()]);
    server.join().unwrap();
}

#[test]
fn a_bytes_frame_over_the_cap_is_a_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let reply = [bytes_frame(&vec![1; BYTES_MAX + 1]), response_line()].concat();
    let server = answer_raw(&sock, reply);
    let mut calls = 0;
    let err = Client::new(&sock)
        .read(show(), |_| {
            calls += 1;
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(err, ClientError::Protocol(_)), "{err}");
    assert_eq!(calls, 0);
    server.join().unwrap();
}

#[test]
fn a_bytes_frame_for_another_request_is_a_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let mut reply = br#"{"frame":"bytes","id":"2","len":2}"#.to_vec();
    reply.extend_from_slice(b"\nab");
    reply.extend_from_slice(&response_line());
    let server = answer_raw(&sock, reply);
    let mut calls = 0;
    let err = Client::new(&sock)
        .read(show(), |_| {
            calls += 1;
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(err, ClientError::Protocol(_)), "{err}");
    assert_eq!(calls, 0);
    server.join().unwrap();
}

#[test]
fn a_bytes_frame_cut_short_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let mut reply = bytes_frame(b"0123456789");
    reply.truncate(reply.len() - 7);
    let server = answer_raw(&sock, reply);
    let err = Client::new(&sock).read(show(), |_| Ok(())).unwrap_err();
    assert!(matches!(err, ClientError::Io(_)), "{err}");
    server.join().unwrap();
}

fn failed_line(message: &str) -> Vec<u8> {
    let error = moor_core::response::ErrorView {
        kind: moor_core::response::ErrorKind::Failed,
        message: message.to_owned(),
        hints: Vec::new(),
        context: Vec::new(),
        detail: None,
    };
    let envelope = serde_json::to_string(&Response::err("artifact-read", error)).unwrap();
    format!("{{\"frame\":\"response\",\"id\":\"1\",\"envelope\":{envelope}}}\n").into_bytes()
}

#[test]
fn read_stream_yields_raw_bytes_across_frames_then_the_response() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let reply = [
        bytes_frame(b"\x00\n\xff"),
        bytes_frame(b"\xc3("),
        response_line(),
    ]
    .concat();
    let server = answer_raw(&sock, reply);
    let mut stream = Client::new(&sock).read_stream(show()).unwrap();
    let (mut got, mut buf) = (Vec::new(), [0_u8; 2]);
    loop {
        let n = stream.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
    }
    assert_eq!(got, b"\x00\n\xff\xc3(");
    assert_eq!(stream.finish().unwrap(), Response::ok("show", None));
    server.join().unwrap();
}

#[test]
fn read_stream_fails_on_an_error_response_after_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let reply = [bytes_frame(b"abc"), failed_line("disk gone")].concat();
    let server = answer_raw(&sock, reply);
    let mut stream = Client::new(&sock).read_stream(show()).unwrap();
    let mut got = Vec::new();
    let err = stream.read_to_end(&mut got).unwrap_err();
    assert!(err.to_string().contains("disk gone"), "{err}");
    let reply = stream.finish().unwrap();
    assert_eq!(reply.error.map(|e| e.message).as_deref(), Some("disk gone"));
    server.join().unwrap();
}

#[test]
fn read_stream_fails_when_the_connection_drops_mid_frame() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let mut reply = bytes_frame(b"0123456789");
    reply.truncate(reply.len() - 7);
    let server = answer_raw(&sock, reply);
    let mut stream = Client::new(&sock).read_stream(show()).unwrap();
    assert!(stream.read_to_end(&mut Vec::new()).is_err());
    assert!(matches!(stream.finish(), Err(ClientError::Io(_))));
    server.join().unwrap();
}

#[test]
fn read_stream_finish_refuses_unread_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let reply = [bytes_frame(b"abcd"), response_line()].concat();
    let server = answer_raw(&sock, reply);
    let mut stream = Client::new(&sock).read_stream(show()).unwrap();
    stream.read_exact(&mut [0_u8; 2]).unwrap();
    assert!(matches!(stream.finish(), Err(ClientError::Protocol(_))));
    server.join().unwrap();
}

#[test]
fn read_stream_refuses_a_bytes_frame_over_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let reply = [bytes_frame(&vec![1; BYTES_MAX + 1]), response_line()].concat();
    let server = answer_raw(&sock, reply);
    let mut stream = Client::new(&sock).read_stream(show()).unwrap();
    assert!(stream.read(&mut [0_u8; 8]).is_err());
    assert!(matches!(stream.finish(), Err(ClientError::Protocol(_))));
    server.join().unwrap();
}

mod redial;
mod remote;
