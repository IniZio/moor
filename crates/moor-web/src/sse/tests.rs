//! Tests for the SSE wire format.

#![allow(clippy::unwrap_used, reason = "tests fail loudly on bad fixtures")]

use moor_core::controller::{parse_server_frame, ServerFrame};
use moor_core::SessionId;

use super::*;

fn record(box_name: Option<&str>, session: Option<SessionId>) -> EventRecord {
    EventRecord {
        cursor: 7,
        box_name: box_name.map(str::to_owned),
        session,
        kind: "idled".to_owned(),
        payload: serde_json::json!({"a": 1}),
        ts: 5,
    }
}

fn data_line(text: &str) -> &str {
    text.lines().nth(1).unwrap().strip_prefix("data: ").unwrap()
}

#[test]
fn event_with_box_and_session() {
    let event = record(Some("t"), Some(SessionId::new(3)));
    let text = event_frame(&event);
    assert_eq!(
        text,
        "id: 7\ndata: {\"frame\":\"event\",\"cursor\":7,\"box\":\"t\",\"session\":3,\"kind\":\"idled\",\"payload\":{\"a\":1},\"ts\":5}\n\n"
    );
    let parsed = parse_server_frame(data_line(&text)).unwrap();
    assert_eq!(
        parsed,
        ServerFrame::Event {
            cursor: 7,
            r#box: Some("t".to_owned()),
            session: Some(SessionId::new(3)),
            kind: "idled".to_owned(),
            payload: event.payload,
            ts: 5,
        }
    );
}

#[test]
fn event_without_box_or_session() {
    let event = record(None, None);
    let text = event_frame(&event);
    assert_eq!(
        text,
        "id: 7\ndata: {\"frame\":\"event\",\"cursor\":7,\"kind\":\"idled\",\"payload\":{\"a\":1},\"ts\":5}\n\n"
    );
    let parsed = parse_server_frame(data_line(&text)).unwrap();
    assert!(matches!(
        parsed,
        ServerFrame::Event {
            cursor: 7,
            r#box: None,
            session: None,
            ts: 5,
            ..
        }
    ));
}

#[test]
fn error_frame_strips_line_breaks() {
    assert_eq!(error_frame("a\r\nb\nc"), "event: error\ndata: a  b c\n\n");
}

#[test]
fn keepalive_is_a_comment() {
    assert_eq!(KEEPALIVE, b": keepalive\n\n");
}

#[test]
fn header_wins_over_query() {
    assert_eq!(start_cursor(Some("5"), Some("9")), 5);
    assert_eq!(start_cursor(Some(" 5 "), None), 5);
}

#[test]
fn query_used_when_header_absent() {
    assert_eq!(start_cursor(None, Some("9")), 9);
}

#[test]
fn unparsable_header_does_not_fall_back() {
    assert_eq!(start_cursor(Some("junk"), Some("9")), 0);
}

#[test]
fn absent_or_garbage_is_zero() {
    assert_eq!(start_cursor(None, None), 0);
    assert_eq!(start_cursor(None, Some("")), 0);
    assert_eq!(start_cursor(None, Some("x1")), 0);
}
