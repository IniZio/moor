#![allow(
    clippy::unwrap_used,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use super::{plain_request, refusal};

const GET: &str = "GET http://example.com/x?a=1 HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: Basic bW9vcjp4\r\nProxy-Connection: keep-alive\r\nConnection: keep-alive, X-Hop\r\nX-Hop: 1\r\nAccept: */*\r\n\r\n";

#[test]
fn a_plain_get_becomes_an_origin_form_request_without_hop_headers_or_credentials() {
    let request = plain_request(GET.as_bytes(), "example.com:80", None).unwrap();
    assert_eq!(
        String::from_utf8(request).unwrap(),
        "GET /x?a=1 HTTP/1.1\r\nHost: example.com\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
}

#[test]
fn a_request_for_another_authority_is_not_rewritten() {
    assert!(plain_request(GET.as_bytes(), "other.example:80", None).is_none());
}

#[test]
fn connection_cannot_drop_host_or_content_length() {
    let head = "POST http://example.com/p HTTP/1.1\r\nHost: example.com\r\nContent-Length: 4\r\nConnection: host, content-length\r\n\r\n";
    let request = plain_request(head.as_bytes(), "example.com:80", None).unwrap();
    let text = String::from_utf8(request).unwrap();
    assert!(text.contains("Host: example.com\r\n"), "{text}");
    assert!(text.contains("Content-Length: 4\r\n"), "{text}");
}

#[test]
fn only_plain_http_absolute_requests_pass() {
    for head in [
        "GET https://example.com/ HTTP/1.1\r\n\r\n",
        "GET /origin HTTP/1.1\r\nHost: example.com\r\n\r\n",
        "get http://example.com/ HTTP/1.1\r\n\r\n",
        "CONNECT example.com:443 HTTP/1.1\r\n\r\n",
        "GET http://example.com/ HTTP/1.1\nHost: x\n\n",
    ] {
        assert!(
            plain_request(head.as_bytes(), "example.com:80", None).is_none(),
            "{head}"
        );
    }
}

#[test]
fn the_refusal_is_a_403_with_its_length() {
    let text = refusal();
    assert!(text.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{text}");
    let body = text.split("\r\n\r\n").nth(1).unwrap();
    assert!(
        text.contains(&format!("Content-Length: {}", body.len())),
        "{text}"
    );
}
