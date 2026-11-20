//! The relay mode: TCP in, a Unix socket out.

use std::path::PathBuf;

use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream, UnixStream};

use crate::{next_accept_pause, say, ACCEPT_PAUSE};

/// Carries each connection accepted on `listener` to the Unix socket at `socket`.
///
/// Bytes are copied both ways until either side closes. A connection whose socket cannot be
/// opened is closed, and the next one tries again, so a Proxy that restarts costs only the
/// connections it held.
pub async fn relay(listener: TcpListener, socket: PathBuf) {
    let mut pause = ACCEPT_PAUSE;
    loop {
        if let Ok((client, _)) = listener.accept().await {
            pause = ACCEPT_PAUSE;
            tokio::spawn(carry(client, socket.clone()));
        } else {
            tokio::time::sleep(pause).await;
            pause = next_accept_pause(pause);
        }
    }
}

/// Copies between `client` and a fresh connection to `socket`.
async fn carry(mut client: TcpStream, socket: PathBuf) {
    match UnixStream::connect(&socket).await {
        Ok(mut proxy) => drop(copy_bidirectional(&mut client, &mut proxy).await),
        Err(e) => say(format_args!("cannot reach {}: {e}", socket.display())),
    }
}
