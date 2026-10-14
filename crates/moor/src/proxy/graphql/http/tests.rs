//! Tests for HTTP framing.

use std::io::Cursor;

use serde_json::Value;

use super::{
    copy_body, read_body, read_head, read_response_head, request_framing, response_framing,
    wants_close, write_refusal, Framing, HttpError, ResponseHead, MAX_HEAD,
};

/// Parses a request head from `s`.
fn head(s: &str) -> Result<super::Head, HttpError> {
    read_head(&mut Cursor::new(s.as_bytes()))
}

/// A response head with the given status and headers.
fn resp(status: u16, headers: &[(&str, &str)]) -> ResponseHead {
    ResponseHead {
        raw: Vec::new(),
        status,
        headers: headers
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect(),
    }
}

/// A request head with the given headers.
fn req(headers: &str) -> Result<Framing, HttpError> {
    request_framing(&head(&format!("POST /graphql HTTP/1.1\r\n{headers}\r\n")).unwrap())
}

const CHUNKED: &[u8] = b"4;ext=1\r\nWiki\r\n5\r\npedia\r\n0\r\nTrail: x\r\n\r\n";

#[test]
fn head_parses_and_keeps_raw_bytes() {
    let text = "POST /graphql HTTP/1.1\r\nHost: api.github.com\r\ncontent-length:  5 \r\n\r\nhello";
    let mut r = Cursor::new(text.as_bytes());
    let h = read_head(&mut r).unwrap();
    assert_eq!(h.method, "POST");
    assert_eq!(h.target, "/graphql");
    assert_eq!(h.raw, text.as_bytes().get(..text.len() - 5).unwrap());
    assert_eq!(h.header("Content-Length"), Some("5"));
    assert_eq!(h.header("HOST"), Some("api.github.com"));
    assert_eq!(h.header("missing"), None);
}

#[test]
fn head_parses_connect() {
    let h =
        head("CONNECT api.github.com:443 HTTP/1.1\r\nHost: api.github.com:443\r\n\r\n").unwrap();
    assert_eq!(h.method, "CONNECT");
    assert_eq!(h.target, "api.github.com:443");
}

#[test]
fn eof_before_any_byte_is_closed() {
    assert!(matches!(head(""), Err(HttpError::Closed)));
    assert!(matches!(head("GET / HT"), Err(HttpError::Malformed)));
}

#[test]
fn malformed_heads_are_rejected() {
    let bad = [
        "GET / HTTP/1.1\r\nA: b\r\n c\r\n\r\n",
        "GET / HTTP/1.1\r\nA: b\r\n\tc\r\n\r\n",
        "GET / HTTP/1.1\r\nNoColon\r\n\r\n",
        "GET / HTTP/1.1\nHost: x\n\n",
        "GET / HTTP/1.1\r\nHost: x\nA: b\r\n\r\n",
        "GET / HTTP/1.1\r\nA: b\0c\r\n\r\n",
        "GET / HTTP/1.1\r\nA: b\rc\r\n\r\n",
        "GET / HTTP/1.1\r\nBad Name: x\r\n\r\n",
        "GET / HTTP/1.1\r\n: x\r\n\r\n",
        "GET /  HTTP/1.1\r\n\r\n",
        "GET / HTTP/2\r\n\r\n",
        "GET /\r\n\r\n",
        "\r\nGET / HTTP/1.1\r\n\r\n",
    ];
    for b in bad {
        assert!(matches!(head(b), Err(HttpError::Malformed)), "{b:?}");
    }
}

#[test]
fn oversized_head_is_too_large() {
    let text = format!("GET / HTTP/1.1\r\nA: {}\r\n\r\n", "x".repeat(MAX_HEAD));
    assert!(matches!(head(&text), Err(HttpError::TooLarge)));
}

#[test]
fn response_head_parses() {
    let mut r = Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n".as_slice());
    let h = read_response_head(&mut r).unwrap();
    assert_eq!(h.status, 200);
    assert_eq!(h.headers.len(), 1);
    let mut bad = Cursor::new(b"HTTP/1.1 20 OK\r\n\r\n".as_slice());
    assert!(matches!(
        read_response_head(&mut bad),
        Err(HttpError::Malformed)
    ));
}

#[test]
fn request_framing_rules() {
    assert_eq!(req("").unwrap(), Framing::Empty);
    assert_eq!(req("Content-Length: 7\r\n").unwrap(), Framing::Length(7));
    assert_eq!(
        req("Content-Length: 7\r\ncontent-length: 7\r\n").unwrap(),
        Framing::Length(7)
    );
    assert_eq!(
        req("Transfer-Encoding: Chunked\r\n").unwrap(),
        Framing::Chunked
    );
    for bad in [
        "Transfer-Encoding: chunked\r\nContent-Length: 3\r\n",
        "Transfer-Encoding: gzip, chunked\r\n",
        "Transfer-Encoding: gzip\r\n",
        "Content-Length: 3\r\nContent-Length: 4\r\n",
        "Content-Length: +3\r\n",
        "Content-Length: 3x\r\n",
        "Content-Length:\r\n",
        "Content-Length: 99999999999999999999999\r\n",
    ] {
        assert!(matches!(req(bad), Err(HttpError::Malformed)), "{bad:?}");
    }
}

#[test]
fn response_framing_rules() {
    let len = [("Content-Length", "4")];
    assert_eq!(
        response_framing(&resp(200, &len), "HEAD").unwrap(),
        Framing::Empty
    );
    assert_eq!(
        response_framing(&resp(204, &len), "GET").unwrap(),
        Framing::Empty
    );
    assert_eq!(
        response_framing(&resp(304, &len), "GET").unwrap(),
        Framing::Empty
    );
    assert_eq!(
        response_framing(&resp(101, &[]), "GET").unwrap(),
        Framing::Empty
    );
    assert_eq!(
        response_framing(&resp(200, &[("transfer-encoding", "chunked")]), "GET").unwrap(),
        Framing::Chunked
    );
    assert_eq!(
        response_framing(&resp(200, &len), "GET").unwrap(),
        Framing::Length(4)
    );
    assert_eq!(
        response_framing(&resp(200, &[]), "GET").unwrap(),
        Framing::UntilClose
    );
}

#[test]
fn length_body_reads_exactly() {
    let mut r = Cursor::new(b"helloEXTRA".as_slice());
    let b = read_body(&mut r, &Framing::Length(5), 100).unwrap();
    assert_eq!(b.raw, b"hello");
    assert_eq!(b.decoded, b"hello");
    assert_eq!(r.position(), 5);
}

#[test]
fn chunked_body_keeps_raw_and_decodes() {
    let b = read_body(&mut Cursor::new(CHUNKED), &Framing::Chunked, 100).unwrap();
    assert_eq!(b.raw, CHUNKED);
    assert_eq!(b.decoded, b"Wikipedia");
}

#[test]
fn cap_is_enforced() {
    let r = &mut Cursor::new(b"helloworld".as_slice());
    assert!(matches!(
        read_body(r, &Framing::Length(10), 9),
        Err(HttpError::TooLarge)
    ));
    assert_eq!(r.position(), 0);
    let r = &mut Cursor::new(CHUNKED);
    assert!(matches!(
        read_body(r, &Framing::Chunked, 8),
        Err(HttpError::TooLarge)
    ));
    assert!(read_body(&mut Cursor::new(CHUNKED), &Framing::Chunked, 9).is_ok());
}

#[test]
fn bad_chunking_is_rejected() {
    for bad in [
        b"zz\r\n".as_slice(),
        b"\r\n",
        b"+4\r\nWiki\r\n0\r\n\r\n",
        b"FFFFFFFFFFFFFFFFFF\r\n",
        b"4\r\nWikiXX0\r\n\r\n",
        b"4\r\nWiki\r\n0\r\n",
        b"4\nWiki\r\n0\r\n\r\n",
    ] {
        let got = read_body(&mut Cursor::new(bad), &Framing::Chunked, 100);
        assert!(matches!(got, Err(HttpError::Malformed)), "{bad:?}");
    }
    let long = format!("4;{}\r\nWiki\r\n0\r\n\r\n", "x".repeat(2000));
    assert!(matches!(
        read_body(&mut Cursor::new(long.as_bytes()), &Framing::Chunked, 100),
        Err(HttpError::TooLarge)
    ));
}

#[test]
fn until_close_cannot_be_buffered() {
    let got = read_body(&mut Cursor::new(b"x".as_slice()), &Framing::UntilClose, 10);
    assert!(matches!(got, Err(HttpError::Malformed)));
}

#[test]
fn copy_body_is_verbatim() {
    let cases: [(&[u8], Framing, usize); 4] = [
        (b"", Framing::Empty, 0),
        (b"helloEXTRA", Framing::Length(5), 5),
        (CHUNKED, Framing::Chunked, CHUNKED.len()),
        (b"anything until close", Framing::UntilClose, 20),
    ];
    for (input, framing, used) in cases {
        let mut out = Vec::new();
        let mut r = Cursor::new(input);
        copy_body(&mut r, &mut out, &framing).unwrap();
        assert_eq!(out, input.get(..used).unwrap(), "{framing:?}");
    }
    let big = vec![7_u8; 100_000];
    let mut out = Vec::new();
    copy_body(
        &mut Cursor::new(big.as_slice()),
        &mut out,
        &Framing::Length(100_000),
    )
    .unwrap();
    assert_eq!(out, big);
}

#[test]
fn truncated_length_body_is_malformed() {
    let got = copy_body(
        &mut Cursor::new(b"hi".as_slice()),
        &mut Vec::new(),
        &Framing::Length(5),
    );
    assert!(matches!(got, Err(HttpError::Malformed)));
}

#[test]
fn refusal_is_json_with_message() {
    let mut out = Vec::new();
    write_refusal(&mut out, 403, "mutation \"x\" not allowed").unwrap();
    let mut r = Cursor::new(out.as_slice());
    let h = read_response_head(&mut r).unwrap();
    assert_eq!(h.status, 403);
    let find = |n: &str| {
        h.headers
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(find("Content-Type"), Some("application/json"));
    assert!(wants_close(&h.headers));
    let framing = response_framing(&h, "POST").unwrap();
    let body = read_body(&mut r, &framing, 1000).unwrap();
    let v: Value = serde_json::from_slice(&body.decoded).unwrap();
    assert_eq!(
        v.get("message").and_then(Value::as_str),
        Some("mutation \"x\" not allowed")
    );
}

#[test]
fn wants_close_reads_tokens() {
    let h = |v: &str| vec![("Connection".to_owned(), v.to_owned())];
    assert!(wants_close(&h("close")));
    assert!(wants_close(&h("Keep-Alive, Close")));
    assert!(!wants_close(&h("keep-alive")));
    assert!(!wants_close(&h("closed")));
    assert!(!wants_close(&[]));
    assert!(wants_close(&[(
        "connection".to_owned(),
        "CLOSE".to_owned()
    )]));
}
