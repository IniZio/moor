//! Ports of a lease's VM, reached through `crabbox tunnel`: one tunnel per lease and port,
//! started by the first connection and shared by the later ones.
//!
//! A tunnel listens on this host's loopback while it runs. It stops 60 s after its last
//! connection ends, before its lease pauses or stops, and when the agent exits.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use moor_client::link::ops::{self, Reply};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use super::{checked, Pool};
use tunnel::{end, listens, start, term, Tunnel};

/// Most ports one lease forwards at once.
const MAX_PORTS: usize = 8;
/// Most connections one lease carries at once.
const MAX_CONNS: usize = 64;
/// How long a tunnel stays up after its last connection ends.
const IDLE: Duration = Duration::from_secs(60);
/// How long crabbox gets to say its listener accepts.
const READY: Duration = Duration::from_secs(30);
/// How long a probe waits for the VM's port to close the connection before it counts as
/// listening: the tunnel closes it at once when nothing listens.
const PROBE: Duration = Duration::from_secs(1);
/// How long crabbox gets to end its ssh after SIGTERM.
const TERM_GRACE: Duration = Duration::from_secs(3);
/// How often a connection waiting on another's tunnel start looks again.
const START_POLL: Duration = Duration::from_millis(25);

/// A lease and a port in its VM.
type Key = (String, u16);

/// The tunnels this agent runs.
static TABLE: LazyLock<Arc<Table>> = LazyLock::new(|| Arc::new(Table::new(IDLE)));

/// What the table holds at a key.
enum Slot {
    /// A connection is starting the tunnel; others wait for it.
    Starting,
    /// The tunnel runs.
    Up(Box<Tunnel>),
}

/// What the table's lock guards.
#[derive(Default)]
struct Inner {
    /// The tunnels by lease and port.
    slots: HashMap<Key, Slot>,
    /// The leases going down, which start no tunnel until they are up again.
    stopping: HashSet<String>,
}

/// The tunnels, and how long each outlives its last connection.
pub(super) struct Table {
    /// The slots; never held while a tunnel starts or stops.
    inner: Mutex<Inner>,
    /// How long a tunnel outlives its last connection.
    idle: Duration,
}

/// A connection's claim on a tunnel; dropping it ends the connection and arms the idle timer.
struct Held {
    /// The tunnel's listener.
    addr: SocketAddr,
    /// The table the tunnel is in.
    table: Arc<Table>,
    /// Its place in the table.
    key: Key,
    /// The tunnel's connection count.
    conns: Arc<AtomicUsize>,
    /// The tunnel's epoch.
    epoch: Arc<AtomicU64>,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.conns.fetch_sub(1, Ordering::SeqCst);
        let seen = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let (table, key, epoch) = (
            Arc::clone(&self.table),
            self.key.clone(),
            Arc::clone(&self.epoch),
        );
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                tokio::time::sleep(table.idle).await;
                table.stop_if_idle(&key, &epoch, seen).await;
            });
        }
    }
}

impl Table {
    /// An empty table whose tunnels stop `idle` after their last connection.
    pub(super) fn new(idle: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            idle,
        }
    }

    /// The tunnel to `port` of `slug`, started when there is none, with one more connection on it.
    async fn hold(
        self: &Arc<Self>,
        (home, pool): (&std::path::Path, &Pool),
        (slug, port): (&str, u16),
    ) -> Result<Held, String> {
        let key = (slug.to_owned(), port);
        loop {
            let mut inner = self.inner.lock().await;
            if inner.stopping.contains(slug) {
                return Err("is going to sleep or stop".to_owned());
            }
            let dead = match inner.slots.get_mut(&key) {
                Some(Slot::Up(t)) => t.exited(),
                _ => false,
            };
            if dead {
                inner.slots.remove(&key);
            }
            let conns: usize = inner
                .slots
                .iter()
                .filter(|((s, _), _)| s == slug)
                .map(|(_, slot)| slot.conns())
                .sum();
            if conns >= MAX_CONNS {
                return Err(format!("already carries {MAX_CONNS} connections"));
            }
            match inner.slots.get(&key) {
                Some(Slot::Starting) => {
                    drop(inner);
                    tokio::time::sleep(START_POLL).await;
                }
                Some(Slot::Up(t)) => {
                    t.conns.fetch_add(1, Ordering::SeqCst);
                    t.epoch.fetch_add(1, Ordering::SeqCst);
                    return Ok(Held {
                        addr: t.addr,
                        table: Arc::clone(self),
                        key,
                        conns: Arc::clone(&t.conns),
                        epoch: Arc::clone(&t.epoch),
                    });
                }
                None => {
                    if inner.slots.keys().filter(|(s, _)| s == slug).count() >= MAX_PORTS {
                        return Err(format!("already forwards {MAX_PORTS} ports"));
                    }
                    inner.slots.insert(key.clone(), Slot::Starting);
                    drop(inner);
                    self.finish_start(&key, start(home, pool, slug, port).await)
                        .await?;
                }
            }
        }
    }

    /// Puts the tunnel `started` in the slot its start held, unless it failed or the lease went
    /// down meanwhile, in which case the tunnel stops.
    async fn finish_start(&self, key: &Key, started: Result<Tunnel, String>) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        inner.slots.remove(key);
        let stopping = inner.stopping.contains(&key.0);
        let refused = match started {
            Ok(tunnel) if !stopping => {
                inner.slots.insert(key.clone(), Slot::Up(Box::new(tunnel)));
                None
            }
            Ok(tunnel) => Some(tunnel),
            Err(e) => {
                drop(inner);
                return Err(e);
            }
        };
        drop(inner);
        match refused {
            Some(tunnel) => {
                end(tunnel).await;
                Err("is going to sleep or stop".to_owned())
            }
            None => Ok(()),
        }
    }

    /// Stops the tunnel at `key` when it is still the one whose `epoch` this is, nothing has
    /// happened on it since `seen`, and no connection is open.
    async fn stop_if_idle(&self, key: &Key, epoch: &Arc<AtomicU64>, seen: u64) {
        let mut inner = self.inner.lock().await;
        let idle = matches!(inner.slots.get(key), Some(Slot::Up(t))
            if Arc::ptr_eq(&t.epoch, epoch)
                && t.epoch.load(Ordering::SeqCst) == seen
                && t.conns.load(Ordering::SeqCst) == 0);
        let gone = idle.then(|| inner.slots.remove(key)).flatten();
        drop(inner);
        if let Some(Slot::Up(tunnel)) = gone {
            end(*tunnel).await;
        }
    }

    /// Stops every tunnel of `slug`, ending its connections, and refuses new ones until
    /// [`Table::allow`].
    pub(super) async fn stop_lease(&self, slug: &str) {
        let mut inner = self.inner.lock().await;
        inner.stopping.insert(slug.to_owned());
        let keys: Vec<Key> = inner
            .slots
            .iter()
            .filter(|((s, _), slot)| s == slug && matches!(slot, Slot::Up(_)))
            .map(|(k, _)| k.clone())
            .collect();
        let gone: Vec<Slot> = keys.iter().filter_map(|k| inner.slots.remove(k)).collect();
        drop(inner);
        for slot in gone {
            if let Slot::Up(tunnel) = slot {
                end(*tunnel).await;
            }
        }
    }

    /// Lets `slug` start tunnels again: its lease is up.
    pub(super) async fn allow(&self, slug: &str) {
        self.inner.lock().await.stopping.remove(slug);
    }

    /// [`kill_all_now`] for this table's tunnels.
    pub(super) fn kill_all_now(&self) {
        for _ in 0..50 {
            if let Ok(inner) = self.inner.try_lock() {
                for slot in inner.slots.values() {
                    if let Slot::Up(t) = slot {
                        term(&t.child);
                    }
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Stops every tunnel.
    pub(super) async fn stop_all(&self) {
        let slots: Vec<Slot> = self
            .inner
            .lock()
            .await
            .slots
            .drain()
            .map(|(_, s)| s)
            .collect();
        for slot in slots {
            if let Slot::Up(tunnel) = slot {
                end(*tunnel).await;
            }
        }
    }
}

impl Slot {
    /// Connections in flight on the tunnel.
    fn conns(&self) -> usize {
        match self {
            Self::Starting => 0,
            Self::Up(t) => t.conns.load(Ordering::SeqCst),
        }
    }
}

/// Stops every tunnel of `slug`: its lease is about to pause or stop.
pub(super) async fn stop_lease(slug: &str) {
    TABLE.stop_lease(slug).await;
}

/// Lets `slug` start tunnels again.
pub(super) async fn allow(slug: &str) {
    TABLE.allow(slug).await;
}

/// Stops every tunnel: the agent is exiting.
pub(super) async fn stop_all() {
    TABLE.stop_all().await;
}

/// Signals every running tunnel's crabbox to stop, without waiting, for an exit that cannot
/// await: an update restarting the agent in place, or a runtime about to drop.
pub fn kill_all_now() {
    TABLE.kill_all_now();
}

/// Serves a port op on `ch`: the stream to `port` of lease `slug`, or with `probe` whether
/// anything listens there; why it was refused.
pub(super) async fn serve<S>(
    table: &Arc<Table>,
    place: (&std::path::Path, &Pool),
    mut ch: S,
    target: (&str, u16),
    probe: bool,
) -> Option<String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let held = match held(table, place, target).await {
        Ok(held) => held,
        Err(reason) => return refuse(ch, reason).await,
    };
    if probe {
        let reply = if listens(held.addr).await {
            Reply::Reachable
        } else {
            Reply::Unreachable
        };
        let _ = ops::write_line(&mut ch, &reply).await;
        return None;
    }
    carry(ch, held.addr).await
}

/// Answers started and carries bytes between `ch` and the tunnel at `addr`; why it could not.
async fn carry<S>(mut ch: S, addr: SocketAddr) -> Option<String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    match TcpStream::connect(addr).await {
        Ok(mut tcp) => {
            if ops::write_line(&mut ch, &Reply::Started).await.is_ok() {
                let _ = tokio::io::copy_bidirectional(&mut ch, &mut tcp).await;
            }
            None
        }
        Err(e) => refuse(ch, format!("cannot reach the tunnel: {e}")).await,
    }
}

/// A connection's claim on the tunnel to `port` of `slug`, when the slug and port are valid.
async fn held(
    table: &Arc<Table>,
    place: (&std::path::Path, &Pool),
    (slug, port): (&str, u16),
) -> Result<Held, String> {
    let slug = checked(slug)?;
    if port == 0 {
        return Err("port 0 is not a port".to_owned());
    }
    table.hold(place, (slug, port)).await
}

/// [`serve`] on this agent's tunnels.
pub(crate) async fn serve_here<S>(
    place: (&std::path::Path, &Pool),
    ch: S,
    target: (&str, u16),
    probe: bool,
) -> Option<String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    serve(&TABLE, place, ch, target, probe).await
}

/// Answers `reason` and closes `ch`; the reason.
async fn refuse<S: AsyncWrite + Unpin + Send>(mut ch: S, reason: String) -> Option<String> {
    let reply = Reply::Refused {
        reason: reason.clone(),
    };
    let _ = ops::write_line(&mut ch, &reply).await;
    Some(reason)
}

#[cfg(test)]
mod tests;
mod tunnel;
