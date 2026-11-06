//! The socket the Box's supervisor holds a connection on while a linked machine forwards its
//! ports: each connection asks the machine's agent to listen, and each connection the agent
//! accepts is named on that connection and waits for the supervisor to dial back for it.

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_client::forward::{Accepted, Carried, Claim, Forward, Forwarding, Local, MAX_PORTS};
use moor_client::link::ops::{self, Op, Reply};
use moor_client::link::Channel;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::Agents;

/// How long an accepted connection waits for the supervisor to dial back before it is closed.
const CLAIM_WITHIN: Duration = Duration::from_secs(30);

/// The listens up now, and the accepted connections waiting for a dial back.
#[derive(Default)]
pub(crate) struct Routes {
    /// Source of listen ids.
    next: AtomicU64,
    /// By machine and listen id.
    by: Mutex<HashMap<(i64, u64), Listen>>,
    /// Accepted connections by number.
    waiting: Waiting<Channel>,
}

/// One listen: its Box ports, and the carrier that names each connection.
struct Listen {
    /// The ports the agent listens on.
    ports: BTreeSet<u16>,
    /// Lines for the supervisor's connection.
    carrier: mpsc::UnboundedSender<Carried>,
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
    /// Adds `ports` for `machine_id` under a fresh listen id; the lines for its carrier.
    fn add(
        self: &Arc<Self>,
        machine_id: i64,
        ports: &[u16],
    ) -> (Route, mpsc::UnboundedReceiver<Carried>) {
        let key = (machine_id, self.next.fetch_add(1, Ordering::Relaxed));
        let (carrier, lines) = mpsc::unbounded_channel();
        let listen = Listen {
            ports: ports.iter().copied().collect(),
            carrier,
        };
        self.by
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, listen);
        let route = Route {
            routes: Arc::clone(self),
            key,
        };
        (route, lines)
    }

    /// The carrier of the listen `accepted` came in on, on `machine_id`'s link.
    fn carrier(
        &self,
        machine_id: i64,
        accepted: Accepted,
    ) -> Option<mpsc::UnboundedSender<Carried>> {
        self.by
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(machine_id, accepted.listen))
            .filter(|l| l.ports.contains(&accepted.port))
            .map(|l| l.carrier.clone())
    }
}

/// Streams held by number until claimed or expired.
pub(crate) struct Waiting<T> {
    /// Source of numbers.
    next: AtomicU64,
    /// The streams.
    held: Mutex<HashMap<u64, T>>,
}

impl<T> Default for Waiting<T> {
    fn default() -> Self {
        Self {
            next: AtomicU64::new(0),
            held: Mutex::default(),
        }
    }
}

impl<T> Waiting<T> {
    /// Holds `stream`; its number.
    fn put(&self, stream: T) -> u64 {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        self.lock().insert(n, stream);
        n
    }

    /// Takes stream `n`, if it still waits.
    fn take(&self, n: u64) -> Option<T> {
        self.lock().remove(&n)
    }

    /// Drops stream `n` once [`CLAIM_WITHIN`] passes unless it was taken.
    async fn expire(&self, n: u64) {
        tokio::time::sleep(CLAIM_WITHIN).await;
        drop(self.take(n));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, T>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
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

/// Serves forward requests and dial backs on `listener` from processes of user `uid` until
/// accepting fails.
///
/// # Errors
/// Returns the error that stopped the listener.
pub async fn serve_forwards(listener: UnixListener, agents: Agents, uid: u32) -> io::Result<()> {
    loop {
        let (conn, _) = listener.accept().await?;
        if conn.peer_cred().is_ok_and(|c| c.uid() == uid) {
            let agents = agents.clone();
            tokio::spawn(async move { local(conn, &agents).await });
        }
    }
}

/// Serves one connection on the socket on this host by its first line.
async fn local(mut conn: UnixStream, agents: &Agents) {
    match ops::read_line::<_, Local>(&mut conn).await {
        Ok(Local::Claim(Claim::Conn { n })) => claim(conn, &agents.routes, n, true).await,
        Ok(Local::Forward(req)) => carry(conn, agents, Ok(req)).await,
        Err(e) => carry(conn, agents, Err(e)).await,
    }
}

/// Serves one supervisor's connection, refused unless `admitted`.
pub(crate) async fn hold<S>(mut conn: S, agents: &Agents, admitted: bool)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let req = match ops::read_line::<_, Forward>(&mut conn).await {
        Ok(_) if !admitted => Err(crate::broker::WRONG_KEY.to_owned()),
        req => req,
    };
    carry(conn, agents, req).await;
}

/// Answers `req` on `conn`, then names each accepted connection on it until either it or the
/// agent's listen stream closes.
async fn carry<S>(mut conn: S, agents: &Agents, req: Result<Forward, String>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let up = match req {
        Ok(req) => up(agents, &req).await,
        Err(e) => Err(e),
    };
    let (mut stream, route, mut lines) = match up {
        Ok(up) => up,
        Err(reason) => {
            let _ = ops::write_line(&mut conn, &Forwarding::Refused { reason }).await;
            return;
        }
    };
    if ops::write_line(&mut conn, &Forwarding::Up).await.is_err() {
        return;
    }
    let (mut read, mut write) = tokio::io::split(conn);
    let (mut a, mut b) = ([0u8; 1], [0u8; 1]);
    loop {
        tokio::select! {
            _ = read.read(&mut a) => break,
            _ = stream.read(&mut b) => break,
            Some(line) = lines.recv() => {
                if ops::write_line(&mut write, &line).await.is_err() {
                    break;
                }
            }
        }
    }
    drop(route);
}

/// Asks `req`'s machine to listen on its Box ports; the listen's stream, routes and carrier
/// lines.
async fn up(
    agents: &Agents,
    req: &Forward,
) -> Result<(Channel, Route, mpsc::UnboundedReceiver<Carried>), String> {
    if req.ports.is_empty() || req.ports.len() > MAX_PORTS {
        return Err(format!("a forward takes 1 to {MAX_PORTS} ports"));
    }
    let machine = req.machine_id;
    let (route, lines) = agents.routes.add(machine, &req.ports);
    let mut stream = agents
        .open(machine)
        .await
        .map_err(|e| format!("machine {machine} is offline: {e}"))?;
    let listen = Op::Listen {
        box_id: req.box_id,
        listen: route.key.1,
        ports: req.ports.clone(),
    };
    ops::write_line(&mut stream, &listen)
        .await
        .map_err(|e| format!("machine {machine} is offline: {e}"))?;
    match ops::read_line(&mut stream).await? {
        Reply::Listening => Ok((stream, route, lines)),
        Reply::Refused { reason } => Err(format!("machine {machine} refused: {reason}")),
        _ => Err(format!("machine {machine} answered something else")),
    }
}

/// Names a connection machine `machine_id`'s agent accepted on its listen's carrier and holds
/// it for the dial back.
pub(crate) async fn accepted(ch: Channel, machine_id: i64, accepted: Accepted, routes: &Routes) {
    let Some(carrier) = routes.carrier(machine_id, accepted) else {
        return;
    };
    let n = routes.waiting.put(ch);
    if carrier
        .send(Carried::Conn {
            n,
            port: accepted.port,
        })
        .is_ok()
    {
        routes.waiting.expire(n).await;
    } else {
        drop(routes.waiting.take(n));
    }
}

/// Pipes `conn`, a dial back for connection `n`, to it, refused unless `admitted`.
pub(crate) async fn claim<S>(mut conn: S, routes: &Routes, n: u64, admitted: bool)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let Some(mut ch) = admitted.then(|| routes.waiting.take(n)).flatten() else {
        return;
    };
    let _ = tokio::io::copy_bidirectional(&mut ch, &mut conn).await;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{Waiting, CLAIM_WITHIN};

    #[tokio::test(start_paused = true)]
    async fn a_connection_no_one_dials_back_for_closes_after_the_wait() {
        let waiting = Waiting::default();
        let (held, mut machine) = tokio::io::duplex(64);
        let n = waiting.put(held);
        let expired = waiting.expire(n);
        tokio::pin!(expired);
        tokio::select! {
            () = &mut expired => panic!("expired early"),
            () = tokio::time::sleep(CLAIM_WITHIN.checked_sub(Duration::from_secs(1)).unwrap()) => {}
        }
        machine.write_all(b"x").await.unwrap();
        expired.await;
        assert_eq!(
            machine.read(&mut [0u8; 8]).await.unwrap(),
            0,
            "the machine side closed"
        );
        assert!(waiting.take(n).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_claimed_connection_outlives_the_wait() {
        let waiting = Waiting::default();
        let (held, mut machine) = tokio::io::duplex(64);
        let n = waiting.put(held);
        let mut claimed = waiting.take(n).unwrap();
        waiting.expire(n).await;
        machine.write_all(b"ok").await.unwrap();
        let mut got = [0u8; 2];
        claimed.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ok");
    }
}
