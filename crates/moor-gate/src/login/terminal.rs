//! The portal's terminal: a Box's agent terminal over a WebSocket the session cookie signs
//! in. It sends the same `attach` the `/front` socket relays, as the session's person, so
//! the controller decides whose Box it is. Binary messages carry the terminal's bytes both
//! ways; a text message from the browser is a new size, and one to it says why the terminal
//! ended.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::SystemTime;

use moor_client::AttachKeys;
use moor_core::name::BoxName;
use moor_core::request::AttachReq;
use oauth2::url::{form_urlencoded, Url};
use serde::Deserialize;
use tungstenite::handshake::derive_accept_key;
use tungstenite::protocol::Role;
use tungstenite::{Message, WebSocket};

use super::{check_csrf, Login};
use crate::http::{header, headers_named, respond, unavailable};
use crate::relay::{deliver, lock, wait, Ws};
use crate::streams::Streams;
use crate::{log, Gate};

/// Where the portal opens a terminal: `?box=<name>&cols=<n>&rows=<n>&csrf=<token>`.
pub(crate) const PATH: &str = "/account/api/box/terminal";

/// A size the browser sends once its terminal is resized.
#[derive(Deserialize)]
struct Size {
    /// Columns.
    cols: u16,
    /// Rows.
    rows: u16,
}

/// Checks the session, the page's origin and its CSRF token, then upgrades and relays the
/// agent's terminal of the Box the query names.
pub(crate) fn serve(
    mut stream: TcpStream,
    leftover: Vec<u8>,
    req: &httparse::Request<'_, '_>,
    (gate, login): (&Gate, Option<&Login>),
    streams: &Streams,
) {
    let Some(login) = login else {
        return respond(&mut stream, "404 Not Found", "text/plain", "not found\n");
    };
    let asked = match asked(req, login) {
        Ok(asked) => asked,
        Err((status, why)) => return respond(&mut stream, status, "text/plain", why),
    };
    let upgrade = header(req, "upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let Some(key) = header(req, "sec-websocket-key").filter(|_| upgrade) else {
        return respond(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            "a WebSocket\n",
        );
    };
    let Some(waiting) = streams.wait() else {
        return unavailable(&mut stream);
    };
    let (person, attach) = asked;
    let opened = gate.controller.clone().acting_as(person).attach(attach);
    drop(waiting);
    let (screen, keys) = match opened {
        Ok(opened) => opened,
        Err(e) => {
            log(&format!(
                "the portal terminal cannot reach the controller: {e}"
            ));
            return unavailable(&mut stream);
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
    let _held = streams.hold(&stream);
    relay(
        WebSocket::from_partially_read(stream, leftover, Role::Server, None),
        screen,
        keys,
    );
}

/// The session's identity and the `attach` it asks for, or the status and reason to refuse.
fn asked(
    req: &httparse::Request<'_, '_>,
    login: &Login,
) -> Result<(moor_core::user::Identity, AttachReq), (&'static str, &'static str)> {
    #[allow(
        clippy::disallowed_methods,
        reason = "session expiry; the store takes the time as an argument"
    )]
    let now = SystemTime::now();
    let session = login
        .session(headers_named(req, "cookie"), now)
        .ok_or(("401 Unauthorized", "sign in again\n"))?;
    if !same_origin(header(req, "origin"), login.public()) {
        return Err(("403 Forbidden", "open the terminal from this site\n"));
    }
    let target = req.path.unwrap_or_default();
    let query: HashMap<String, String> = target
        .split_once('?')
        .map(|(_, q)| form_urlencoded::parse(q.as_bytes()).into_owned().collect())
        .unwrap_or_default();
    if !query.get("csrf").is_some_and(|c| check_csrf(&session, c)) {
        return Err(("403 Forbidden", "open the terminal from this site\n"));
    }
    let size = |name: &str| {
        query
            .get(name)
            .and_then(|n| n.parse::<u16>().ok())
            .filter(|n| *n > 0)
    };
    let wanted = query
        .get("box")
        .and_then(|b| BoxName::parse_allow_bare(b).ok());
    let (Some(name), Some(cols), Some(rows)) = (wanted, size("cols"), size("rows")) else {
        return Err(("400 Bad Request", "name a Box and a size\n"));
    };
    let attach = AttachReq {
        target: name.into(),
        cols,
        rows,
        shell: false,
    };
    Ok((session.person.identity, attach))
}

/// Whether the `Origin` header names the gate's own public origin. A browser always sends it
/// on a WebSocket, so its absence is refused as well.
fn same_origin(origin: Option<&str>, public: &str) -> bool {
    let parse = |s: &str| Url::parse(s).ok().map(|u| u.origin());
    match (origin.and_then(parse), parse(public)) {
        (Some(got), Some(own)) => got.is_tuple() && got == own,
        _ => false,
    }
}

/// Relays the screen to the browser and its keys and sizes to the agent, until either ends.
fn relay(ws: WebSocket<TcpStream>, screen: impl Read + Send, mut keys: AttachKeys) {
    let Ok(probe) = ws.get_ref().try_clone() else {
        return;
    };
    if probe.set_nonblocking(true).is_err() {
        return;
    }
    let ws = std::sync::Mutex::new(ws);
    std::thread::scope(|scope| {
        scope.spawn(|| show(screen, &ws, &probe));
        take(&ws, &probe, &mut keys);
        let _ = keys.detach();
    });
}

/// Agent to browser, until the attach ends; then says why and closes.
fn show(mut screen: impl Read, ws: &Ws, probe: &TcpStream) {
    let mut buf = vec![0; 64 * 1024];
    let why = loop {
        match screen.read(&mut buf) {
            Ok(0) => break "the terminal closed".to_owned(),
            Ok(n) => {
                let chunk = buf.get(..n).unwrap_or_default().to_vec();
                if !deliver(ws, probe, Some(Message::binary(chunk))) {
                    return;
                }
            }
            Err(e) => break e.to_string(),
        }
    };
    let ended = serde_json::json!({ "ended": why }).to_string();
    if deliver(ws, probe, Some(Message::text(ended))) && lock(ws).close(None).is_ok() {
        deliver(ws, probe, None);
    }
    let _ = probe.shutdown(Shutdown::Read);
}

/// Browser to agent: keystrokes and sizes, until the browser closes or the socket fails.
fn take(ws: &Ws, probe: &TcpStream, keys: &mut AttachKeys) {
    loop {
        if !wait(probe, rustix::event::PollFlags::IN) {
            return;
        }
        loop {
            let read = lock(ws).read();
            let sent = match read {
                Ok(Message::Binary(bytes)) => keys.send(&bytes),
                Ok(Message::Text(text)) => match serde_json::from_str::<Size>(&text) {
                    Ok(size) if size.cols > 0 && size.rows > 0 => keys.resize(size.cols, size.rows),
                    _ => Ok(()),
                },
                Ok(Message::Close(_)) => return,
                Ok(_) => Ok(()),
                Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    break
                }
                Err(_) => return,
            };
            if sent.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::same_origin;

    #[test]
    fn only_the_public_origin_may_open_a_terminal() {
        let public = "https://moor.example.com";
        assert!(same_origin(Some("https://moor.example.com"), public));
        assert!(same_origin(Some("https://moor.example.com:443"), public));
        assert!(!same_origin(Some("https://evil.example.com"), public));
        assert!(!same_origin(Some("http://moor.example.com"), public));
        assert!(!same_origin(
            Some("https://moor.example.com.evil.com"),
            public
        ));
        assert!(!same_origin(Some("null"), public));
        assert!(!same_origin(None, public));
    }
}
