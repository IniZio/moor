//! The socket the Box's supervisor holds a connection on while a linked machine forwards its
//! ports: each connection asks the machine's agent to listen, and each connection the agent
//! accepts is piped to the host port the request paired with it.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use moor_client::forward::{Accepted, Forward, Forwarding, MAX_PORTS};
use moor_client::link::ops::{self, Op, Reply};
use moor_client::link::Channel;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::{TcpStream, UnixListener};

use crate::Agents;

/// The host port of each machine port, per listen up now.
#[derive(Default)]
pub(crate) struct Routes {
    /// Source of listen ids.
    next: AtomicU64,
    /// By machine and listen id: machine port to host port.
    by: Mutex<HashMap<(i64, u64), BTreeMap<u16, u16>>>,
}

/// One listen's routes; dropping it removes them.
struct Route {
    /// Where they are.
    routes: Arc<Routes>,
    /// Their key.
    key: (i64, u64),
}

impl Drop for Route {
    fn drop(&mut self) {
        self.routes
            .by
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

impl Routes {
    /// Adds `ports` for `machine_id` under a fresh listen id.
    fn add(self: &Arc<Self>, machine_id: i64, ports: &[(u16, u16)]) -> Route {
        let key = (machine_id, self.next.fetch_add(1, Ordering::Relaxed));
        self.by
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, ports.iter().copied().collect());
        Route {
            routes: Arc::clone(self),
            key,
        }
    }

    /// The host port for `accepted` on `machine_id`'s link.
    fn host_port(&self, machine_id: i64, accepted: Accepted) -> Option<u16> {
        self.by
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(machine_id, accepted.listen))
            .and_then(|ports| ports.get(&accepted.port).copied())
    }
}

/// Listens beside the runner's `socket` to the controller for the supervisors' forward
/// requests, readable only by this user; the listener and this user's uid.
///
/// # Errors
/// The socket cannot be bound.
pub fn bind_forwards(socket: &Path) -> Result<(UnixListener, u32), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let path = socket.with_file_name(moor_client::forward::SOCKET);
    let failed = |e: std::io::Error| format!("cannot listen on {}: {e}", path.display());
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(failed(e)),
        _ => {}
    }
    let listener = UnixListener::bind(&path).map_err(failed)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(failed)?;
    let uid = std::fs::metadata(&path).map_err(failed)?.uid();
    Ok((listener, uid))
}

/// Serves forward requests on `listener` from processes of user `uid` until accepting fails.
///
/// # Errors
/// Returns the error that stopped the listener.
pub async fn serve_forwards(listener: UnixListener, agents: Agents, uid: u32) -> io::Result<()> {
    loop {
        let (conn, _) = listener.accept().await?;
        if conn.peer_cred().is_ok_and(|c| c.uid() == uid) {
            let agents = agents.clone();
            tokio::spawn(async move { hold(conn, &agents, true).await });
        }
    }
}

/// Serves one supervisor's connection, refused unless `admitted`: the forwards stay up until
/// either it or the agent's listen stream closes.
pub(crate) async fn hold<S>(mut conn: S, agents: &Agents, admitted: bool)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let up = match ops::read_line::<_, Forward>(&mut conn).await {
        Ok(_) if !admitted => Err(crate::broker::WRONG_KEY.to_owned()),
        Ok(req) => up(agents, &req).await,
        Err(e) => Err(e),
    };
    let (mut stream, route) = match up {
        Ok(up) => up,
        Err(reason) => {
            let _ = ops::write_line(&mut conn, &Forwarding::Refused { reason }).await;
            return;
        }
    };
    if ops::write_line(&mut conn, &Forwarding::Up).await.is_err() {
        return;
    }
    let (mut a, mut b) = ([0u8; 1], [0u8; 1]);
    tokio::select! {
        _ = conn.read(&mut a) => {}
        _ = stream.read(&mut b) => {}
    }
    drop(route);
}

/// Asks `req`'s machine to listen on its Box ports; the listen's stream and routes.
async fn up(agents: &Agents, req: &Forward) -> Result<(Channel, Route), String> {
    if req.ports.is_empty() || req.ports.len() > MAX_PORTS {
        return Err(format!("a forward takes 1 to {MAX_PORTS} ports"));
    }
    let machine = req.machine_id;
    let route = agents.routes.add(machine, &req.ports);
    let mut stream = agents
        .open(machine)
        .await
        .map_err(|e| format!("machine {machine} is offline: {e}"))?;
    let listen = Op::Listen {
        box_id: req.box_id,
        listen: route.key.1,
        ports: req.ports.iter().map(|(p, _)| *p).collect(),
    };
    ops::write_line(&mut stream, &listen)
        .await
        .map_err(|e| format!("machine {machine} is offline: {e}"))?;
    match ops::read_line(&mut stream).await? {
        Reply::Listening => Ok((stream, route)),
        Reply::Refused { reason } => Err(format!("machine {machine} refused: {reason}")),
        _ => Err(format!("machine {machine} answered something else")),
    }
}

/// Pipes a connection machine `machine_id`'s agent accepted to the host port routed for it.
pub(crate) async fn accepted(
    mut ch: Channel,
    machine_id: i64,
    accepted: Accepted,
    routes: &Routes,
) {
    let Some(host) = routes.host_port(machine_id, accepted) else {
        return;
    };
    if let Ok(mut tcp) = TcpStream::connect((Ipv4Addr::LOCALHOST, host)).await {
        let _ = tokio::io::copy_bidirectional(&mut ch, &mut tcp).await;
    }
}
