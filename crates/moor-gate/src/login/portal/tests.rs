//! The head an artifact download is sent with.

use std::io::Read as _;
use std::net::{TcpListener, TcpStream};

use super::*;

/// The head `head` writes for an artifact of `content_type` named `name`.
fn head_of(name: &str, content_type: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    let record = ArtifactView {
        name: name.to_owned(),
        version: 1,
        kind: "screenshot".to_owned(),
        content_type: content_type.to_owned(),
        size: 3,
        sha256: String::new(),
        created_at: String::new(),
    };
    head(&mut server, &record).unwrap();
    drop(server);
    let mut out = String::new();
    client.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn a_raster_image_is_shown_inline_and_never_sniffed() {
    let head = head_of("after.png", "image/png");
    assert!(head.contains("Content-Type: image/png\r\n"), "{head}");
    assert!(head.contains("Content-Disposition: inline\r\n"), "{head}");
    assert!(head.contains("X-Content-Type-Options: nosniff"), "{head}");
    assert!(head.contains("Content-Security-Policy: sandbox"), "{head}");
}

#[test]
fn svg_and_everything_else_is_a_download() {
    for (name, kind) in [
        ("a.svg", "image/svg+xml"),
        ("a.html", "text/html"),
        ("a b.png", "text/plain"),
    ] {
        let head = head_of(name, kind);
        assert!(
            head.contains("Content-Type: application/octet-stream\r\n"),
            "{head}"
        );
        assert!(head.contains("attachment; filename=\"a"), "{head}");
    }
}
