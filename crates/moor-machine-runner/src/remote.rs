//! The runner's listener for a controller in another pod: each connection opens with a hello
//! naming the links broker or port forwards and carrying the machine service key.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use moor_client::link::ops;
use moor_client::link::reach::{Hello, Keyed, HELLO_V};
use tokio::net::{TcpListener, TcpStream};

use crate::Agents;

/// How long a connection may take to send its hello.
const HELLO_WITHIN: Duration = Duration::from_secs(10);

/// Listens on `addr` for a controller in another pod and serves it in the background.
///
/// # Errors
/// The address cannot be bound.
pub async fn listen_remote(addr: SocketAddr, agents: Agents, key: String) -> Result<(), String> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| format!("cannot listen for the controller on {addr}: {e}"))?;
    if let Ok(addr) = listener.local_addr() {
        eprintln!("moor-machine-runner taking controller dials on {addr}");
    }
    tokio::spawn(async move {
        if let Err(e) = serve_remote(listener, agents, key).await {
            eprintln!("moor-machine-runner: the controller listener stopped: {e}");
        }
    });
    Ok(())
}

/// Serves the broker and forwards on `listener` to connections whose hello carries `key`,
/// until accepting fails.
async fn serve_remote(listener: TcpListener, agents: Agents, key: String) -> io::Result<()> {
    let key: Arc<str> = key.into();
    loop {
        let (conn, _) = listener.accept().await?;
        let (agents, key) = (agents.clone(), Arc::clone(&key));
        tokio::spawn(async move { greet(conn, &agents, &key).await });
    }
}

/// Reads `conn`'s hello and serves what it asks for; one that sends none in time is dropped.
async fn greet(mut conn: TcpStream, agents: &Agents, key: &str) {
    let _ = conn.set_nodelay(true);
    let read = ops::read_line::<_, Hello>(&mut conn);
    let Ok(Ok(hello)) = tokio::time::timeout(HELLO_WITHIN, read).await else {
        return;
    };
    match hello {
        Hello::Links(keyed) => crate::broker::carry(conn, agents, admits(&keyed, key)).await,
        Hello::Forwards(keyed) => crate::forwards::hold(conn, agents, admits(&keyed, key)).await,
    }
}

/// Whether `keyed` speaks this version and carries `key`; the keys' digests are compared in
/// constant time, so timing says nothing of the key.
fn admits(keyed: &Keyed, key: &str) -> bool {
    use sha2::{Digest as _, Sha256};
    use subtle::ConstantTimeEq as _;
    let same = Sha256::digest(keyed.key.as_bytes()).ct_eq(&Sha256::digest(key.as_bytes()));
    keyed.v == HELLO_V && bool::from(same)
}
