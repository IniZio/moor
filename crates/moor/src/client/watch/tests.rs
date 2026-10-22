use std::io::{self, Cursor, Write};

use moor_core::controller::{ClientFrame, FrameErrorKind, ServerFrame, PROTOCOL};
use moor_core::BoxName;

use super::*;

fn hello() -> String {
    format!(r#"{{"frame":"hello","protocol":{PROTOCOL}}}"#)
}

fn filter() -> WatchFilter {
    WatchFilter {
        boxes: Some(vec![BoxName::parse_allow_bare("a").unwrap()]),
        kinds: Some(vec!["copied".to_owned()]),
    }
}

fn run(
    server: &[String],
    follow: bool,
    out: &mut impl Write,
) -> (Result<Option<String>, SessionError>, String) {
    let input = format!("{}\n", server.join("\n"));
    let mut sent = Vec::new();
    let mut since = 7;
    let result = watch_session(
        Cursor::new(input),
        &mut sent,
        &mut since,
        &filter(),
        follow,
        out,
    );
    (result, String::from_utf8(sent).unwrap())
}

fn event(cursor: i64) -> String {
    format!(
        r#"{{"frame":"event","cursor":{cursor},"box":"a","kind":"copied","payload":{{}},"ts":1}}"#
    )
}

#[test]
fn sends_hello_then_watch_with_filter_and_follow() {
    let mut out = Vec::new();
    let (result, sent) = run(&[hello()], true, &mut out);
    assert!(result.is_ok());
    let watch = ClientFrame::Watch {
        since: 7,
        boxes: filter().boxes,
        kinds: filter().kinds,
        follow: true,
    };
    let hello = ClientFrame::Hello { protocol: PROTOCOL };
    assert_eq!(sent, format!("{}{}", hello.to_line(), watch.to_line()));
}

#[test]
fn copies_frames_until_eof() {
    let mut out = Vec::new();
    let (result, _) = run(&[hello(), event(8), event(9)], false, &mut out);
    assert!(result.is_ok());
    let text = String::from_utf8(out).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(text.contains(r#""cursor":8"#) && text.contains(r#""cursor":9"#));
}

#[test]
fn caught_up_frame_is_printed_after_the_events() {
    let caught_up = ServerFrame::CaughtUp { cursor: 9 }.to_line();
    let mut out = Vec::new();
    let (result, _) = run(
        &[hello(), event(8), event(9), caught_up.trim_end().to_owned()],
        false,
        &mut out,
    );
    assert!(result.is_ok());
    let text = String::from_utf8(out).unwrap();
    let mut frames = text.lines();
    assert!(frames.next().is_some_and(|l| l.contains(r#""cursor":8"#)));
    assert!(frames.next().is_some_and(|l| l.contains(r#""cursor":9"#)));
    assert_eq!(format!("{}\n", frames.next().unwrap()), caught_up);
    assert!(frames.next().is_none());
}

#[test]
fn error_frame_is_an_error_and_not_printed() {
    let error = ServerFrame::Error {
        id: None,
        kind: FrameErrorKind::SlowClient,
        message: "too slow".to_owned(),
    };
    let mut out = Vec::new();
    let (result, _) = run(&[hello(), event(8), error.to_line()], false, &mut out);
    assert!(matches!(
        result,
        Err(SessionError::Client(ClientError::Protocol(m))) if m.contains("too slow")
    ));
    assert_eq!(String::from_utf8(out).unwrap().lines().count(), 1);
}

struct Broken;

impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn broken_output_ends_quietly() {
    let (result, _) = run(&[hello(), event(8), event(9)], true, &mut Broken);
    assert!(result.is_ok());
}
