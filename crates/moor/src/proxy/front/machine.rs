//! The machine service. A plain `http://` request a Box sends to the service's host name goes
//! straight to the service's loopback address, never to nono, carrying the Box and the machine
//! it holds in `X-Moor-*` headers that only this front sets. A Box that holds no slot is passed
//! on too, marked with why, so its agent sees a tool error and not an auth failure. A CONNECT to
//! that host is refused.

use std::io::Write;
use std::net::{Shutdown, TcpStream};
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use moor_core::config::MachineService;
use moor_core::BoxId;

use super::{plain, refuse, splice, Head, TARGET_TIMEOUT};

/// Prefix of the headers the front owns; the Box's own are dropped.
const OWN_PREFIX: &str = "x-moor-";
/// Body of the answer when the service does not accept the connection.
const UNAVAILABLE_BODY: &str = r#"{"message":"moor: the machine service is not answering"}"#;

/// Reads what the Box holds now.
pub(crate) type HeldFn = Arc<dyn Fn() -> Hold + Send + Sync>;

/// What a Box holds of the machine service.
pub(crate) enum Hold {
    /// A slot on a machine.
    Held(Held),
    /// No slot. A client treats a 401 or 403 as a call for OAuth and drops the server's tools
    /// for good, so the service is told instead.
    Missing {
        /// The Box.
        box_id: BoxId,
        /// Why, as one line the agent reads.
        reason: String,
    },
}

/// The machine a Box holds, as the service learns it.
pub(crate) struct Held {
    /// The Box.
    pub(crate) box_id: BoxId,
    /// The machine's name.
    pub(crate) name: String,
    /// The machine's ssh target.
    pub(crate) ssh: String,
    /// The Box's port slot on the machine.
    pub(crate) slot: u16,
    /// rsync patterns a sync to the machine leaves alone.
    pub(crate) sync_exclude: Vec<String>,
}

/// The service one Box's front serves itself.
pub(crate) struct MachineRoute {
    /// Its host name and loopback address.
    pub(crate) service: MachineService,
    /// Read on every request, since a machine may be attached or freed while the front runs.
    pub(crate) held: HeldFn,
}

impl MachineRoute {
    /// True when `head` names the service's host, as a CONNECT or a plain request.
    pub(super) fn aims_at(&self, head: &[u8]) -> bool {
        target_host(head).is_some_and(|host| self.service.serves(host))
    }

    /// Sends a plain request to the service with the Box's identity, then relays both ways.
    pub(super) fn serve(&self, client: UnixStream, head: &Head) {
        let Some(plain) = plain::parse(&head.bytes) else {
            return refuse(client);
        };
        let Some(request) = marked(&plain.request, &(self.held)()) else {
            return refuse(client);
        };
        let Ok(mut upstream) = TcpStream::connect_timeout(&self.service.upstream, TARGET_TIMEOUT)
        else {
            return unavailable(client);
        };
        if upstream.write_all(&request).is_err() || upstream.write_all(&head.rest).is_err() {
            return;
        }
        splice(client, upstream);
    }
}

/// The host, without port, that a request line targets: the authority of a CONNECT or of an
/// absolute `http://` target.
fn target_host(head: &[u8]) -> Option<&str> {
    let line = head.split(|b| *b == b'\r').next()?;
    let target = std::str::from_utf8(line).ok()?.split(' ').nth(1)?;
    let authority = match target.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("http://") => {
            let rest = target.get(7..)?;
            rest.get(..rest.find(['/', '?']).unwrap_or(rest.len()))?
        }
        _ => target,
    };
    Some(match authority.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => authority,
    })
}

/// `request` without any `X-Moor-*` header, with the front's own added.
fn marked(request: &[u8], slot: &Hold) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(request)
        .ok()?
        .strip_suffix("\r\n\r\n")?;
    let own = match slot {
        Hold::Held(held) => vec![
            ("X-Moor-Box", held.box_id.get().to_string()),
            ("X-Moor-Machine", held.name.clone()),
            ("X-Moor-Machine-Ssh", held.ssh.clone()),
            ("X-Moor-Machine-Slot", held.slot.to_string()),
            (
                "X-Moor-Machine-Exclude",
                serde_json::to_string(&held.sync_exclude).ok()?,
            ),
        ],
        Hold::Missing { box_id, reason } => vec![
            ("X-Moor-Box", box_id.get().to_string()),
            ("X-Moor-Machine-Missing", reason.clone()),
        ],
    };
    if own
        .iter()
        .any(|(_, v)| v.bytes().any(|b| b.is_ascii_control()))
    {
        return None;
    }
    let theirs = |line: &&str| {
        line.get(..OWN_PREFIX.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(OWN_PREFIX))
    };
    let mut out = String::new();
    for line in text.split("\r\n").filter(|l| !theirs(l)) {
        out.push_str(line);
        out.push_str("\r\n");
    }
    for (name, value) in own {
        for part in [name, ": ", &value, "\r\n"] {
            out.push_str(part);
        }
    }
    out.push_str("\r\n");
    Some(out.into_bytes())
}

/// Answers 502 and closes.
fn unavailable(mut client: UnixStream) {
    let reply = format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{UNAVAILABLE_BODY}",
        UNAVAILABLE_BODY.len()
    );
    let _ = client.write_all(reply.as_bytes());
    let _ = client.shutdown(Shutdown::Write);
}
