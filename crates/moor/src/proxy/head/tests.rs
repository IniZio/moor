//! Tests for the bounded request-head reader.

use std::fmt::Write as _;
use std::io::{self, Cursor, Read};

use super::{read_head, HeadError, MAX_HEADERS, MAX_HEAD_BYTES};

/// Reader whose first read fails with a connection reset.
struct Reset;

impl Read for Reset {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::ConnectionReset))
    }
}

#[test]
fn valid_get_returns_head_and_body_prefix() {
    let raw = b"GET /x HTTP/1.1\r\nHost: a\r\nAccept: */*\r\n\r\nbody";
    let (head, rest) = read_head(&mut Cursor::new(&raw[..])).unwrap().unwrap();
    assert_eq!(head.method, "GET");
    assert_eq!(head.target, "/x");
    assert_eq!(head.header_pairs(), vec![("Host", "a"), ("Accept", "*/*")]);
    assert_eq!(rest, b"body");
    assert!(!head.expects_continue());
}

#[test]
fn debug_output_omits_target_and_header_values() {
    let raw = b"GET /t/SENTINELNONCE123/v1/x HTTP/1.1\r\nHost: a\r\nAuthorization: Bearer SENTINELNONCE123\r\n\r\n";
    let (head, _) = read_head(&mut Cursor::new(&raw[..])).unwrap().unwrap();
    let shown = format!("{head:?}");
    assert!(!shown.contains("SENTINELNONCE123"), "{shown}");
    assert!(shown.contains("GET"), "{shown}");
    assert!(shown.contains("Authorization"), "{shown}");
}

#[test]
fn http_1_0_head_is_accepted() {
    let raw = b"GET / HTTP/1.0\r\n\r\n";
    let (head, rest) = read_head(&mut Cursor::new(&raw[..])).unwrap().unwrap();
    assert_eq!(head.method, "GET");
    assert!(rest.is_empty());
}

#[test]
fn unterminated_oversize_head_is_too_large() {
    let mut raw = b"GET / HTTP/1.1\r\nX: ".to_vec();
    raw.resize(MAX_HEAD_BYTES + 100, b'a');
    let err = read_head(&mut Cursor::new(raw)).unwrap_err();
    assert!(matches!(err, HeadError::TooLarge));
    assert_eq!(err.status(), Some(431));
}

#[test]
fn too_many_headers_is_rejected() {
    let mut raw = String::from("GET / HTTP/1.1\r\n");
    for i in 0..=MAX_HEADERS {
        write!(raw, "H{i}: v\r\n").unwrap();
    }
    raw.push_str("\r\n");
    let err = read_head(&mut Cursor::new(raw.into_bytes())).unwrap_err();
    assert!(matches!(err, HeadError::TooManyHeaders));
    assert_eq!(err.status(), Some(431));
}

#[test]
fn garbage_is_malformed() {
    let err = read_head(&mut Cursor::new(b"\x01\x02 not http\r\n\r\n".to_vec())).unwrap_err();
    assert!(matches!(err, HeadError::Malformed));
    assert_eq!(err.status(), Some(400));
}

#[test]
fn non_utf8_header_value_is_malformed() {
    let raw = b"GET / HTTP/1.1\r\nX: \xff\xfe\r\n\r\n".to_vec();
    let err = read_head(&mut Cursor::new(raw)).unwrap_err();
    assert!(matches!(err, HeadError::Malformed));
}

#[test]
fn empty_input_is_clean_eof() {
    assert!(read_head(&mut Cursor::new(Vec::new())).unwrap().is_none());
}

#[test]
fn reset_before_any_byte_is_clean_eof() {
    assert!(read_head(&mut Reset).unwrap().is_none());
}

#[test]
fn eof_mid_head_is_malformed() {
    let err = read_head(&mut Cursor::new(b"GET / HTTP/1.1\r\nHost: a\r\n".to_vec())).unwrap_err();
    assert!(matches!(err, HeadError::Malformed));
}

#[test]
fn io_error_has_no_status() {
    let err = HeadError::Io(io::Error::from(io::ErrorKind::BrokenPipe));
    assert_eq!(err.status(), None);
    let HeadError::Io(inner) = err else {
        unreachable!("constructed as Io above");
    };
    assert_eq!(inner.kind(), io::ErrorKind::BrokenPipe);
}

#[test]
fn expect_continue_is_case_insensitive() {
    let raw = b"POST /u HTTP/1.1\r\nexpect: 100-Continue\r\n\r\n";
    let (head, _) = read_head(&mut Cursor::new(&raw[..])).unwrap().unwrap();
    assert!(head.expects_continue());
}
