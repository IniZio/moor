#![allow(clippy::panic, clippy::indexing_slicing, reason = "test assertions")]

use super::*;

fn err_parts(line: &str) -> (Option<String>, FrameErrorKind) {
    match parse_frame(line) {
        Err(ServerFrame::Error { id, kind, .. }) => (id, kind),
        other => panic!("expected error frame, got {other:?}"),
    }
}

#[test]
fn parses_hello() {
    let got = parse_frame(r#"{"frame":"hello","protocol":1}"#);
    assert_eq!(got, Ok(ClientFrame::Hello { protocol: PROTOCOL }));
}

#[test]
fn parses_request() {
    let got = parse_frame(r#"{"frame":"request","id":"1","request":{"cmd":"version"}}"#);
    assert!(matches!(
        got,
        Ok(ClientFrame::Request { ref id, request: Request::Version {} }) if id == "1"
    ));
}

#[test]
fn unknown_field_is_malformed() {
    let (id, kind) = err_parts(r#"{"frame":"hello","protocol":1,"extra":true}"#);
    assert_eq!(id, None);
    assert_eq!(kind, FrameErrorKind::Malformed);
}

#[test]
fn malformed_recovers_id() {
    let (id, kind) = err_parts(r#"{"frame":"request","id":"7","request":{"cmd":"nope"}}"#);
    assert_eq!(id.as_deref(), Some("7"));
    assert_eq!(kind, FrameErrorKind::Malformed);
}

#[test]
fn non_object_has_no_id() {
    assert_eq!(err_parts("[1,2]").0, None);
    assert_eq!(err_parts("not json").0, None);
    assert_eq!(err_parts(r#"{"id":5}"#).0, None);
}

#[test]
fn server_hello_shape() {
    let line = ServerFrame::Hello { protocol: PROTOCOL }.to_line();
    assert_eq!(line, "{\"frame\":\"hello\",\"protocol\":1}\n");
}

#[test]
fn server_error_shape() {
    let with_id = ServerFrame::Error {
        id: Some("3".into()),
        kind: FrameErrorKind::UnsupportedProtocol,
        message: "m".into(),
    };
    assert_eq!(
        with_id.to_line(),
        "{\"frame\":\"error\",\"id\":\"3\",\"kind\":\"unsupported-protocol\",\"message\":\"m\"}\n"
    );
    let without = ServerFrame::Error {
        id: None,
        kind: FrameErrorKind::HelloRequired,
        message: "m".into(),
    };
    assert_eq!(
        without.to_line(),
        "{\"frame\":\"error\",\"kind\":\"hello-required\",\"message\":\"m\"}\n"
    );
}
