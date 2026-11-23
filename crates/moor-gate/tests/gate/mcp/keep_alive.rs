//! A connection that a reverse proxy keeps alive: every request on it is answered by the
//! handler its own path names, or the gate tells the proxy to stop reusing the connection.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::Value;

use super::mcp;

/// One response the gate wrote on a connection.
struct Answer {
    status: u16,
    /// Whether it said `Connection: close`.
    closing: bool,
    body: String,
}

/// The next response read off `reader`; none when the gate closed the connection instead.
fn response(reader: &mut BufReader<TcpStream>) -> Option<Answer> {
    let mut status = String::new();
    if reader.read_line(&mut status).ok()? == 0 {
        return None;
    }
    let code = status.split(' ').nth(1).unwrap().parse().unwrap();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        headers.push((name.to_ascii_lowercase(), value.trim().to_owned()));
    }
    let length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map_or(0, |(_, v)| v.parse().unwrap());
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let closing = headers
        .iter()
        .any(|(name, v)| name == "connection" && v.eq_ignore_ascii_case("close"));
    Some(Answer {
        status: code,
        closing,
        body: String::from_utf8(body).unwrap(),
    })
}

#[test]
fn a_request_after_mcp_on_a_kept_alive_connection_reaches_its_own_handler() {
    let m = mcp();
    let stream = TcpStream::connect(&m.world.gate).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    write!(
        writer,
        "POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: 2\r\n\r\n{{}}"
    )
    .unwrap();
    let first = response(&mut reader).unwrap();
    assert_eq!(first.status, 401);

    let _ = write!(writer, "GET /.well-known/moor HTTP/1.1\r\nHost: x\r\n\r\n");
    match response(&mut reader) {
        Some(second) => {
            assert_eq!(second.status, 200, "{}", second.body);
            let doc: Value = serde_json::from_str(&second.body).unwrap();
            assert_eq!(doc["resource"], m.url());
        }
        None => assert!(
            first.closing,
            "the gate closed the connection without saying Connection: close"
        ),
    }
}
