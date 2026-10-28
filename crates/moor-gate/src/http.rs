//! The gate's HTTP surface: the metadata document, the `/front` upgrade, and 404 for the rest.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use tungstenite::handshake::derive_accept_key;
use tungstenite::protocol::Role;
use tungstenite::WebSocket;

use crate::{log, relay, Gate};

/// Longest request head the gate reads.
const HEAD_MAX: usize = 16 * 1024;

/// How long a client may take to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Serves one HTTP connection.
pub(crate) fn handle(stream: TcpStream, gate: &Gate) {
    if stream.set_read_timeout(Some(HEAD_TIMEOUT)).is_err() {
        return;
    }
    let mut reader = BufReader::new(stream);
    let Some(head) = read_head(&mut reader) else {
        return;
    };
    let leftover = reader.buffer().to_vec();
    let mut stream = reader.into_inner();
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut req = httparse::Request::new(&mut headers);
    if !matches!(req.parse(&head), Ok(httparse::Status::Complete(_))) {
        respond(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            "bad request\n",
        );
        return;
    }
    let path = req.path.unwrap_or_default();
    let path = path.split('?').next().unwrap_or_default();
    match (req.method, path) {
        (Some("GET"), "/.well-known/moor") => {
            let doc = serde_json::json!({
                "issuer": gate.verifier.issuer(),
                "client_id": gate.verifier.client_id(),
                "scopes": ["openid", "email", "offline_access"],
            });
            respond(&mut stream, "200 OK", "application/json", &doc.to_string());
        }
        (Some("GET"), p) if p.starts_with(crate::mcp::METADATA_PATH) && gate.mcp.is_some() => {
            let doc = gate
                .mcp
                .as_ref()
                .map(|mcp| mcp.metadata(gate.verifier.issuer()))
                .unwrap_or_default();
            respond(&mut stream, "200 OK", "application/json", &doc);
        }
        (_, "/mcp" | "/mcp/") if gate.mcp.is_some() => {
            if let Some(mcp) = &gate.mcp {
                let mut prefix = head.clone();
                prefix.extend_from_slice(&leftover);
                mcp.serve(stream, prefix, gate);
            }
        }
        (Some("GET"), "/front") => front(stream, leftover, &req, gate),
        _ => respond(&mut stream, "404 Not Found", "text/plain", "not found\n"),
    }
}

/// Checks the bearer token, then upgrades and relays to the controller's front socket.
fn front(mut stream: TcpStream, leftover: Vec<u8>, req: &httparse::Request<'_, '_>, gate: &Gate) {
    let person = header(req, "authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| "send an access token as Authorization: Bearer <token>".to_owned())
        .and_then(|token| gate.verifier.verify(token.trim()));
    let person = match person {
        Ok(person) => person,
        Err(why) => {
            let _ = write!(
                stream,
                "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{why}\n",
                why.len() + 1
            );
            return;
        }
    };
    let upgrade = header(req, "upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let Some(key) = header(req, "sec-websocket-key").filter(|_| upgrade) else {
        respond(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            "/front is a WebSocket\n",
        );
        return;
    };
    let unix = match UnixStream::connect(&gate.front_socket) {
        Ok(unix) => unix,
        Err(e) => {
            log(&format!(
                "the controller's front socket is unreachable: {e}"
            ));
            respond(
                &mut stream,
                "502 Bad Gateway",
                "text/plain",
                "the controller is unreachable\n",
            );
            return;
        }
    };
    let accept = derive_accept_key(key.as_bytes());
    let switched = write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if switched.is_err() || stream.set_read_timeout(None).is_err() {
        return;
    }
    relay::run(
        WebSocket::from_partially_read(stream, leftover, Role::Server, None),
        &unix,
        &person,
    );
}

/// The request head up to and including the blank line, or none when it is cut off or too long.
fn read_head(reader: &mut BufReader<TcpStream>) -> Option<Vec<u8>> {
    let mut head = Vec::new();
    loop {
        let before = head.len();
        let n = reader.read_until(b'\n', &mut head).ok()?;
        let line = head.get(before..)?;
        if n == 0 || head.len() > HEAD_MAX {
            return None;
        }
        if line == b"\r\n" || line == b"\n" {
            return Some(head);
        }
    }
}

/// The value of header `name`, matched without case.
fn header<'a>(req: &'a httparse::Request<'_, '_>, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .and_then(|h| std::str::from_utf8(h.value).ok())
}

/// Writes a whole response and lets the connection close.
fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}
