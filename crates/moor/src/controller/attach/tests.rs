use std::io::{BufRead, BufReader, Cursor, Write};
use std::os::unix::net::UnixListener;

use super::*;

fn env() -> Env {
    Env::default()
}

#[test]
fn round_trips_hello_and_response() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let mut w = stream;
        writeln!(w, r#"{{"frame":"ok"}}"#).unwrap();
        line
    });
    let mut out = Vec::new();
    let mut err = Vec::new();
    let input = Cursor::new(b"{\"frame\":\"hello\"}\n".to_vec());
    let code = attach(&env(), Some(&sock), input, &mut out, &mut err);
    assert_eq!(code, 0);
    assert_eq!(server.join().unwrap(), "{\"frame\":\"hello\"}\n");
    assert_eq!(String::from_utf8(out).unwrap(), "{\"frame\":\"ok\"}\n");
    assert!(err.is_empty());
}

#[test]
fn unreachable_socket_names_path() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("missing.sock");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = attach(
        &env(),
        Some(&sock),
        Cursor::new(Vec::new()),
        &mut out,
        &mut err,
    );
    assert_eq!(code, 1);
    let text = String::from_utf8(err).unwrap();
    assert!(text.starts_with("moor: controller at "), "{text}");
    assert!(text.contains(&sock.display().to_string()), "{text}");
    assert!(text.contains("is unreachable"), "{text}");
    assert!(out.is_empty());
}

#[test]
fn server_sees_eof_after_input_ends() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut got = Vec::new();
        stream.read_to_end(&mut got).unwrap();
        // Close only after seeing EOF, so attach can return.
        got
    });
    let code = attach(
        &env(),
        Some(&sock),
        Cursor::new(b"abc\n".to_vec()),
        Vec::new(),
        Vec::new(),
    );
    assert_eq!(code, 0);
    assert_eq!(server.join().unwrap(), b"abc\n");
}
