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
    let got = parse_frame(&format!(r#"{{"frame":"hello","protocol":{PROTOCOL}}}"#));
    assert_eq!(got, Ok(ClientFrame::Hello { protocol: PROTOCOL }));
}

#[test]
fn parses_request() {
    let got = parse_frame(r#"{"frame":"request","id":"1","request":{"cmd":"version"}}"#);
    assert!(matches!(
        got,
        Ok(ClientFrame::Request { ref id, ref request }) if id == "1" && **request == Request::Version {}
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
    assert_eq!(
        line,
        format!("{{\"frame\":\"hello\",\"protocol\":{PROTOCOL}}}\n")
    );
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

#[test]
fn parses_watch() {
    let got = parse_frame(r#"{"frame":"watch","since":0}"#);
    assert_eq!(
        got,
        Ok(ClientFrame::Watch {
            since: 0,
            boxes: None,
            kinds: None,
            follow: true,
        })
    );
}

#[test]
fn watch_with_filter_and_no_follow_round_trips() {
    let watch = ClientFrame::Watch {
        since: 4,
        boxes: Some(vec![BoxName::parse_allow_bare("fix-login").expect("name")]),
        kinds: Some(vec!["idled".to_owned()]),
        follow: false,
    };
    let line = watch.to_line();
    assert!(line.contains(r#""boxes":["fix-login"]"#), "{line}");
    assert!(line.contains(r#""follow":false"#), "{line}");
    assert_eq!(parse_frame(&line), Ok(watch));
}

#[test]
fn default_watch_omits_filter_and_follow() {
    let watch = ClientFrame::Watch {
        since: 3,
        boxes: None,
        kinds: None,
        follow: true,
    };
    assert_eq!(watch.to_line(), "{\"frame\":\"watch\",\"since\":3}\n");
}

#[test]
fn watch_filter_matches() {
    let name = |s: &str| BoxName::parse_allow_bare(s).expect("name");
    let all = WatchFilter::default();
    assert!(all.matches(Some("a"), "idled"));
    let by_box = WatchFilter {
        boxes: Some(vec![name("a")]),
        kinds: None,
    };
    assert!(by_box.matches(Some("a"), "idled"));
    assert!(!by_box.matches(Some("b"), "idled"));
    let by_kind = WatchFilter {
        boxes: None,
        kinds: Some(vec!["idled".to_owned()]),
    };
    assert!(by_kind.matches(Some("b"), "idled"));
    assert!(!by_kind.matches(Some("b"), "started"));
    let both = WatchFilter {
        boxes: Some(vec![name("a")]),
        kinds: Some(vec!["idled".to_owned()]),
    };
    assert!(both.matches(Some("a"), "idled"));
    assert!(!both.matches(Some("a"), "started"));
    assert!(!both.matches(Some("b"), "idled"));
    let none = WatchFilter {
        boxes: Some(vec![]),
        kinds: None,
    };
    assert!(!none.matches(Some("a"), "idled"));
}

#[test]
fn watch_filter_matches_boxless_event() {
    let name = |s: &str| BoxName::parse_allow_bare(s).expect("name");
    let kinds = Some(vec!["idled".to_owned()]);
    assert!(WatchFilter::default().matches(None, "idled"));
    assert!(WatchFilter {
        boxes: None,
        kinds: kinds.clone()
    }
    .matches(None, "idled"));
    assert!(!WatchFilter { boxes: None, kinds }.matches(None, "started"));
    assert!(!WatchFilter {
        boxes: Some(vec![name("a")]),
        kinds: None
    }
    .matches(None, "idled"));
}

#[test]
fn caught_up_shape() {
    let frame = ServerFrame::CaughtUp { cursor: 42 };
    assert_eq!(frame.to_line(), "{\"frame\":\"caught-up\",\"cursor\":42}\n");
    assert_eq!(parse_server_frame(&frame.to_line()).expect("parses"), frame);
}

#[test]
fn watch_unknown_field_is_malformed() {
    let (id, kind) = err_parts(r#"{"frame":"watch","since":0,"extra":1}"#);
    assert_eq!(id, None);
    assert_eq!(kind, FrameErrorKind::Malformed);
}

#[test]
fn event_frame_serializes() {
    let frame = ServerFrame::Event {
        cursor: 7,
        r#box: Some("fix-login".to_owned()),
        session: None,
        kind: "idled".to_owned(),
        payload: serde_json::json!({"a": 1}),
        ts: 123,
    };
    let got = serde_json::to_string(&frame).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        got,
        r#"{"frame":"event","cursor":7,"box":"fix-login","kind":"idled","payload":{"a":1},"ts":123}"#
    );
}

#[test]
fn event_frame_golden_with_session() {
    let line = ServerFrame::Event {
        cursor: 7,
        r#box: Some("t".into()),
        session: Some(SessionId::new(3)),
        kind: "idled".into(),
        payload: serde_json::json!({"a": 1}),
        ts: 5,
    }
    .to_line();
    assert_eq!(
        line,
        "{\"frame\":\"event\",\"cursor\":7,\"box\":\"t\",\"session\":3,\"kind\":\"idled\",\"payload\":{\"a\":1},\"ts\":5}\n"
    );
    let parsed = parse_server_frame(&line).expect("parses");
    assert!(matches!(
        parsed,
        ServerFrame::Event { session: Some(s), .. } if s == SessionId::new(3)
    ));
}

#[test]
fn event_frame_golden_without_session() {
    let line = ServerFrame::Event {
        cursor: 7,
        r#box: Some("t".into()),
        session: None,
        kind: "idled".into(),
        payload: serde_json::json!({"a": 1}),
        ts: 5,
    }
    .to_line();
    assert_eq!(
        line,
        "{\"frame\":\"event\",\"cursor\":7,\"box\":\"t\",\"kind\":\"idled\",\"payload\":{\"a\":1},\"ts\":5}\n"
    );
    let parsed = parse_server_frame(&line).expect("parses");
    assert!(matches!(parsed, ServerFrame::Event { session: None, .. }));
}

#[test]
fn event_frame_golden_without_box_or_session() {
    let line = ServerFrame::Event {
        cursor: 7,
        r#box: None,
        session: None,
        kind: "idled".into(),
        payload: serde_json::json!({"a": 1}),
        ts: 5,
    }
    .to_line();
    assert_eq!(
        line,
        "{\"frame\":\"event\",\"cursor\":7,\"kind\":\"idled\",\"payload\":{\"a\":1},\"ts\":5}\n"
    );
    let parsed = parse_server_frame(&line).expect("parses");
    assert!(matches!(
        parsed,
        ServerFrame::Event {
            r#box: None,
            session: None,
            ..
        }
    ));
}

#[test]
fn protocol_is_seven_and_old_hello_still_parses() {
    assert_eq!(PROTOCOL, 7);
    let got = parse_frame(r#"{"frame":"hello","protocol":1}"#);
    assert_eq!(got, Ok(ClientFrame::Hello { protocol: 1 }));
}

#[test]
fn slow_client_kind_serializes() {
    let got = serde_json::to_string(&FrameErrorKind::SlowClient).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(got, r#""slow-client""#);
}

#[test]
fn store_unavailable_kind_serializes() {
    let got =
        serde_json::to_string(&FrameErrorKind::StoreUnavailable).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(got, r#""store-unavailable""#);
}

#[test]
fn client_frames_round_trip_through_parse_frame() {
    let hello = ClientFrame::Hello { protocol: PROTOCOL };
    assert_eq!(parse_frame(&hello.to_line()), Ok(hello));
    let request = ClientFrame::Request {
        id: "9".into(),
        request: Box::new(Request::Version {}),
    };
    assert_eq!(parse_frame(&request.to_line()), Ok(request));
    let watch = ClientFrame::Watch {
        since: 3,
        boxes: None,
        kinds: None,
        follow: true,
    };
    assert_eq!(parse_frame(&watch.to_line()), Ok(watch));
}

#[test]
fn server_frames_round_trip_through_parse_server_frame() {
    let frames = [
        ServerFrame::Hello { protocol: PROTOCOL },
        ServerFrame::Progress {
            id: "1".into(),
            message: "fetching".into(),
        },
        ServerFrame::Response {
            id: "1".into(),
            envelope: Response::ok("version", None),
        },
        ServerFrame::Error {
            id: Some("2".into()),
            kind: FrameErrorKind::Malformed,
            message: "m".into(),
        },
        ServerFrame::Error {
            id: None,
            kind: FrameErrorKind::HelloRequired,
            message: "m".into(),
        },
        ServerFrame::Event {
            cursor: 7,
            r#box: Some("t".into()),
            session: Some(SessionId::new(3)),
            kind: "idled".into(),
            payload: serde_json::json!({"a": 1}),
            ts: 123,
        },
        ServerFrame::Event {
            cursor: 8,
            r#box: Some("t".into()),
            session: None,
            kind: "idled".into(),
            payload: serde_json::json!({}),
            ts: 124,
        },
        ServerFrame::Bytes {
            id: "1".into(),
            len: BYTES_MAX,
        },
    ];
    for frame in frames {
        let parsed = parse_server_frame(&frame.to_line()).expect("parses");
        assert_eq!(parsed, frame);
    }
}

#[test]
fn progress_shape() {
    let line = ServerFrame::Progress {
        id: "4".into(),
        message: "step".into(),
    }
    .to_line();
    assert_eq!(
        line,
        "{\"frame\":\"progress\",\"id\":\"4\",\"message\":\"step\"}\n"
    );
}

/// Splits a written stream into its bytes frames: (id, payload) per frame.
fn bytes_frames(mut wire: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut frames = Vec::new();
    while let Some(end) = wire.iter().position(|b| *b == b'\n') {
        let (line, rest) = wire.split_at(end + 1);
        let line = std::str::from_utf8(line).unwrap_or_else(|e| panic!("{e}"));
        let Ok(ServerFrame::Bytes { id, len }) = parse_server_frame(line.trim_end()) else {
            panic!("not a bytes frame: {line}");
        };
        let (payload, rest) = rest.split_at(len);
        frames.push((id, payload.to_vec()));
        wire = rest;
    }
    assert!(wire.is_empty(), "{} trailing bytes", wire.len());
    frames
}

#[test]
fn bytes_frame_is_a_json_line_then_raw_bytes() {
    let mut wire = Vec::new();
    write_bytes(&mut wire, "3", b"\n\xff\x00").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        wire,
        b"{\"frame\":\"bytes\",\"id\":\"3\",\"len\":3}\n\n\xff\x00"
    );
}

#[test]
fn write_bytes_never_puts_more_than_the_cap_in_one_frame() {
    let data: Vec<u8> = (0..=2 * BYTES_MAX)
        .map(|i| u8::try_from(i % 251).unwrap_or_default())
        .collect();
    let mut wire = Vec::new();
    write_bytes(&mut wire, "7", &data).unwrap_or_else(|e| panic!("{e}"));
    let frames = bytes_frames(&wire);
    let lens: Vec<usize> = frames.iter().map(|(_, p)| p.len()).collect();
    assert_eq!(lens, [64 * 1024, 64 * 1024, 1]);
    assert!(frames.iter().all(|(id, _)| id == "7"));
    let joined: Vec<u8> = frames.into_iter().flat_map(|(_, p)| p).collect();
    assert_eq!(joined, data);
}

#[test]
fn request_frame_json_is_unchanged_by_boxing() {
    let line = r#"{"frame":"request","id":"2","request":{"cmd":"open-host-session","workdir":"/w","parent":4}}"#;
    let frame = parse_frame(line).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(frame.to_line().trim_end(), line);
}
