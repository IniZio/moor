//! Tests for the GitHub token check against a loopback server.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test code")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

use moor_core::secret::Secret;

use super::GitHub;
use crate::credentials::CheckError;

/// Serves one request with `status`; returns the base URL and the request head it got.
fn serve(status: u16) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0_u8; 4096];
        let n = stream.read(&mut buf).unwrap();
        tx.send(String::from_utf8_lossy(&buf[..n]).into_owned())
            .unwrap();
        let body = "{}";
        let reply = format!(
            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(reply.as_bytes()).unwrap();
    });
    (base, rx)
}

fn token(text: &str) -> Secret {
    Secret::new(text.as_bytes().to_vec())
}

#[test]
fn a_200_from_get_user_takes_the_token_sent_as_a_bearer_with_a_user_agent() {
    let (base, head) = serve(200);
    assert!(GitHub::new(&base).check(&token("gho_good")).is_ok());
    let head = head.recv().unwrap().to_ascii_lowercase();
    assert!(head.starts_with("get /user http/1.1"), "{head}");
    assert!(head.contains("authorization: bearer gho_good"), "{head}");
    assert!(head.contains("user-agent: moor-slack"), "{head}");
}

#[test]
fn a_401_or_403_refuses_and_anything_else_leaves_github_unasked() {
    for status in [401, 403] {
        let (base, _head) = serve(status);
        let got = GitHub::new(&base).check(&token("gho_bad"));
        let want = CheckError::Refused(format!("GitHub answered HTTP {status} for it"));
        assert_eq!(got, Err(want));
    }
    for status in [500, 404, 503] {
        let (base, _head) = serve(status);
        let got = GitHub::new(&base).check(&token("gho_x"));
        let want = CheckError::Unavailable(format!("GitHub answered HTTP {status}"));
        assert_eq!(got, Err(want), "{status}");
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    drop(listener);
    let got = GitHub::new(&base).check(&token("gho_secret_value"));
    let Err(CheckError::Unavailable(why)) = got else {
        panic!("{got:?}");
    };
    assert!(!why.contains("gho_secret_value"), "{why}");
}

#[test]
fn only_one_visible_word_passes_the_format_check() {
    assert!(GitHub::check_format(&token("gho_16C7e42F292c6912E7710c838347Ae178B4a")).is_ok());
    for bad in ["", "two words", "tab\there", "line\nbreak", "ünï"] {
        assert!(GitHub::check_format(&token(bad)).is_err(), "{bad:?}");
    }
}
