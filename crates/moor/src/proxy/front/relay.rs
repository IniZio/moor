//! A front that relays each connection, unread, to the shared egress Proxy on this host, so a
//! Box's tunnel reaches it when the Box cannot dial the Proxy itself.

use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};

use super::socket::Handler;
use super::{splice, Front, FrontSocket, TARGET_TIMEOUT};

impl Front {
    /// Binds `socket`, replacing a stale file, and relays each connection to `to`.
    pub(crate) fn relay(socket: FrontSocket, to: SocketAddr) -> Result<Self, String> {
        let handler: Handler = Arc::new(move |client| {
            if let Ok(upstream) = TcpStream::connect_timeout(&to, TARGET_TIMEOUT) {
                splice(client, upstream);
            }
        });
        let listening = socket.listen(true, &handler)?;
        Ok(Self {
            socket,
            handler,
            listening: Mutex::new(listening),
        })
    }
}
