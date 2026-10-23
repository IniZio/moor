//! The front's own network namespace, which has nothing but its loopback, so the front has no
//! route out. The outer shim reaches both sides of it: sockets it made before it unshared stay
//! in the host's network, and sockets it makes after are in the front's. So it:
//!
//! - starts the egress helper (see the `egress` module) in the host's network before it
//!   unshares, and listens on `127.0.0.1` inside the front's for the front's proxy port,
//!   passing each connection to the helper;
//! - listens, before it unshares, on the host's `127.0.0.1` alone for each of the front's
//!   `ports`, and joins each connection to the same port on the front's `127.0.0.1`.

use std::io;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::thread;

use netlink_packet_core::{
    NetlinkHeader, NetlinkMessage, NetlinkPayload, NLM_F_ACK, NLM_F_REQUEST,
};
use netlink_packet_route::link::{LinkFlags, LinkMessage};
use netlink_packet_route::RouteNetlinkMessage;
use netlink_sys::{protocols::NETLINK_ROUTE, Socket, SocketAddr};

use super::egress;

/// The index of the loopback interface in a new network namespace.
const LOOPBACK: u32 = 1;

/// What the outer shim holds of the host's network before it unshares.
pub(super) struct Outside {
    /// The egress helper, in the host's network.
    helper: Child,
    /// The outer shim's end of the socket pair connections pass to the helper over.
    to_helper: UnixStream,
    /// A listener on the host's `127.0.0.1` for each of the front's ports.
    inbound: Vec<(u16, TcpListener)>,
}

impl Outside {
    /// Starts the egress helper and listens on the host's `127.0.0.1` for each of `ports`;
    /// call before the outer shim leaves the host's network.
    pub(super) fn prepare(ports: &[u16]) -> Result<Self, String> {
        let inbound = ports
            .iter()
            .map(|&port| {
                TcpListener::bind(("127.0.0.1", port))
                    .map(|listener| (port, listener))
                    .map_err(|e| format!("cannot listen on the host's 127.0.0.1:{port}: {e}"))
            })
            .collect::<Result<_, _>>()?;
        let (to_helper, helper_end) =
            UnixStream::pair().map_err(|e| format!("cannot make the egress socket pair: {e}"))?;
        let helper = Command::new("/proc/self/exe")
            .args(["front-shim", "--egress"])
            .arg(std::process::id().to_string())
            .stdin(Stdio::from(OwnedFd::from(helper_end)))
            .stdout(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start the egress helper: {e}"))?;
        Ok(Self {
            helper,
            to_helper,
            inbound,
        })
    }

    /// Serves the front, whose network namespace this process is now in with its loopback up:
    /// listens on its `127.0.0.1` for its proxy port and starts passing connections both ways;
    /// returns the proxy port.
    pub(super) fn serve(self) -> Result<u16, String> {
        let proxy = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| format!("cannot listen for the front's egress proxy: {e}"))?;
        let port = proxy
            .local_addr()
            .map_err(|e| format!("cannot read the egress proxy port: {e}"))?
            .port();
        let Self {
            helper: _helper,
            to_helper,
            inbound,
        } = self;
        spawn("moor-front-egress", move || {
            for conn in proxy.incoming().flatten() {
                let _ = egress::pass(&to_helper, &conn);
            }
        })?;
        for (port, listener) in inbound {
            spawn("moor-front-inbound", move || {
                for client in listener.incoming().flatten() {
                    let _ = thread::Builder::new()
                        .name("moor-front-inbound-conn".to_owned())
                        .spawn(move || {
                            if let Ok(front) = TcpStream::connect(("127.0.0.1", port)) {
                                splice(client, front);
                            }
                        });
                }
            })?;
        }
        Ok(port)
    }
}

/// Runs `work` on a thread named `name`.
fn spawn(name: &str, work: impl FnOnce() + Send + 'static) -> Result<(), String> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(work)
        .map(drop)
        .map_err(|e| format!("cannot start {name}: {e}"))
}

/// Copies both ways between `a` and `b` until each side has closed.
pub(super) fn splice(a: TcpStream, b: TcpStream) {
    let (Ok(mut a_rx), Ok(mut b_tx)) = (a.try_clone(), b.try_clone()) else {
        return;
    };
    let up = thread::spawn(move || {
        let _ = io::copy(&mut a_rx, &mut b_tx);
        let _ = b_tx.shutdown(Shutdown::Write);
    });
    let (mut a, mut b) = (a, b);
    let _ = io::copy(&mut b, &mut a);
    let _ = a.shutdown(Shutdown::Write);
    let _ = up.join();
}

/// Brings up the loopback of the network namespace this process is in, which gives it
/// `127.0.0.1` and `::1`.
pub(super) fn loopback_up() -> Result<(), String> {
    let fail = |e: io::Error| format!("cannot bring the front's loopback up: {e}");
    let mut link = LinkMessage::default();
    link.header.index = LOOPBACK;
    link.header.flags = LinkFlags::Up;
    link.header.change_mask = LinkFlags::Up;
    let mut header = NetlinkHeader::default();
    header.flags = NLM_F_REQUEST | NLM_F_ACK;
    header.sequence_number = 1;
    let mut request = NetlinkMessage::new(
        header,
        NetlinkPayload::from(RouteNetlinkMessage::SetLink(link)),
    );
    request.finalize();
    let mut sent = vec![0_u8; request.buffer_len()];
    request.serialize(&mut sent);
    let mut socket = Socket::new(NETLINK_ROUTE).map_err(fail)?;
    socket.bind_auto().map_err(fail)?;
    socket.connect(&SocketAddr::new(0, 0)).map_err(fail)?;
    socket.send(&sent, 0).map_err(fail)?;
    let mut answer = vec![0_u8; 4096];
    let read = socket.recv(&mut &mut answer[..], 0).map_err(fail)?;
    let reply =
        NetlinkMessage::<RouteNetlinkMessage>::deserialize(answer.get(..read).unwrap_or_default())
            .map_err(|e| format!("cannot read the loopback answer: {e}"))?;
    match reply.payload {
        NetlinkPayload::Error(e) if e.code.is_some() => Err(fail(e.to_io())),
        _ => Ok(()),
    }
}
