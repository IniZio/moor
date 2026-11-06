//! Where the controller reaches a machine runner, as a link row names it.
//!
//! A socket path on this host, beside which the runner's own sockets sit, or `host:port`, a
//! listener that serves a connection only after a [`Hello`] carrying the machine service key.

use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::net::{TcpStream, UnixStream};
use tokio_util::either::Either;

use super::ops;

/// How long a connect to a runner across the network may take, unless the caller says.
pub const CONNECT_WITHIN: Duration = Duration::from_secs(10);
/// The hello's version.
pub const HELLO_V: u32 = 1;

/// A machine runner the controller reaches.
#[derive(Clone, PartialEq, Eq)]
pub enum Runner {
    /// On this host: its socket to the controller.
    Local(PathBuf),
    /// Across the network.
    Remote {
        /// `host:port`.
        addr: String,
        /// The machine service key it checks.
        key: String,
    },
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(socket) => f.debug_tuple("Local").field(socket).finish(),
            Self::Remote { addr, .. } => f
                .debug_struct("Remote")
                .field("addr", addr)
                .field("key", &"<redacted>")
                .finish(),
        }
    }
}

/// The first line of a connection to a runner across the network: what it asks for, and the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Hello {
    /// The links broker; an [`Ask`](super::broker::Ask) follows.
    Links(Keyed),
    /// Port forwards; a [`Forward`](crate::forward::Forward) follows.
    Forwards(Keyed),
    /// A dial back for a connection a forwards carrier named; its bytes follow.
    Conn(Dial),
}

/// A dial back's version, key and connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dial {
    /// [`HELLO_V`].
    pub v: u32,
    /// The machine service key.
    pub key: String,
    /// The connection's number, as the carrier named it.
    pub n: u64,
}

/// A hello's version and key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keyed {
    /// [`HELLO_V`].
    pub v: u32,
    /// The machine service key.
    pub key: String,
}

/// What a connection to a runner is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// The links broker.
    Links,
    /// Port forwards.
    Forwards,
    /// The dial back for forwarded connection `n`.
    Conn(u64),
}

impl Service {
    /// The runner's socket for it on this host.
    const fn socket(self) -> &'static str {
        match self {
            Self::Links => super::broker::SOCKET,
            Self::Forwards | Self::Conn(_) => crate::forward::SOCKET,
        }
    }

    /// The hello that asks for it with `key`.
    fn hello(self, key: &str) -> Hello {
        let keyed = Keyed {
            v: HELLO_V,
            key: key.to_owned(),
        };
        match self {
            Self::Links => Hello::Links(keyed),
            Self::Forwards => Hello::Forwards(keyed),
            Self::Conn(n) => Hello::Conn(Dial {
                v: keyed.v,
                key: keyed.key,
                n,
            }),
        }
    }
}

/// An open connection to a runner.
pub type Conn = Either<UnixStream, TcpStream>;

impl Runner {
    /// The runner at `addr`: a path on this host, else `host:port`, which needs `key`.
    ///
    /// # Errors
    /// A runner across the network with no key to give it.
    pub fn at(addr: &str, key: Option<&str>) -> Result<Self, String> {
        if addr.starts_with('/') {
            return Ok(Self::Local(PathBuf::from(addr)));
        }
        let key = key.filter(|k| !k.is_empty()).ok_or_else(|| {
            format!("the machine runner at {addr} needs the machine service key, MOOR_MACHINE_SERVICE_KEY")
        })?;
        Ok(Self::Remote {
            addr: addr.to_owned(),
            key: key.to_owned(),
        })
    }

    /// This runner with its `host:port` looked up, so connecting to it does no name lookup;
    /// this blocks on the lookup.
    ///
    /// # Errors
    /// The name has no address.
    pub fn looked_up(self) -> Result<Self, String> {
        let Self::Remote { addr, key } = self else {
            return Ok(self);
        };
        if addr.parse::<SocketAddr>().is_ok() {
            return Ok(Self::Remote { addr, key });
        }
        let found = addr
            .to_socket_addrs()
            .map_err(|e| unreachable(&addr, &e))?
            .next();
        let found = found.ok_or_else(|| format!("the machine runner at {addr} has no address"))?;
        Ok(Self::Remote {
            addr: found.to_string(),
            key,
        })
    }

    /// A connection for `service`, its hello sent.
    ///
    /// # Errors
    /// The runner cannot be reached.
    pub async fn connect(&self, service: Service) -> Result<Conn, String> {
        self.connect_within(service, CONNECT_WITHIN).await
    }

    /// [`Self::connect`], giving up on a runner across the network after `within`.
    ///
    /// # Errors
    /// The runner cannot be reached.
    pub async fn connect_within(&self, service: Service, within: Duration) -> Result<Conn, String> {
        match self {
            Self::Local(socket) => {
                let path = socket.with_file_name(service.socket());
                let failed = |e: &io::Error| unreachable(&path.display(), e);
                let mut conn = UnixStream::connect(&path).await.map_err(|e| failed(&e))?;
                if let Service::Conn(n) = service {
                    let claim = crate::forward::Claim::Conn { n };
                    ops::write_line(&mut conn, &claim)
                        .await
                        .map_err(|e| failed(&e))?;
                }
                Ok(Either::Left(conn))
            }
            Self::Remote { addr, key } => {
                let failed = |e: &io::Error| unreachable(addr, e);
                let mut tcp = tokio::time::timeout(within, TcpStream::connect(addr))
                    .await
                    .map_err(|_| failed(&io::ErrorKind::TimedOut.into()))?
                    .map_err(|e| failed(&e))?;
                let _ = tcp.set_nodelay(true);
                // A runner pod gone without a FIN or RST must read as gone within half a minute.
                let probes = socket2::TcpKeepalive::new()
                    .with_time(Duration::from_secs(10))
                    .with_interval(Duration::from_secs(5))
                    .with_retries(3);
                socket2::SockRef::from(&tcp)
                    .set_tcp_keepalive(&probes)
                    .map_err(|e| failed(&e))?;
                ops::write_line(&mut tcp, &service.hello(key))
                    .await
                    .map_err(|e| failed(&e))?;
                Ok(Either::Right(tcp))
            }
        }
    }
}

/// Why the runner at `at` was not reached.
fn unreachable(at: &dyn std::fmt::Display, e: &io::Error) -> String {
    format!("the machine runner at {at} is unreachable: {e}")
}

/// A blocking connection to a runner, of either kind.
pub use socket2::Socket as StdConn;

/// `conn` as a blocking connection, for a caller off any runtime.
///
/// # Errors
/// The socket cannot be taken off the runtime.
pub fn into_blocking(conn: Conn) -> io::Result<StdConn> {
    let conn = match conn {
        Either::Left(s) => StdConn::from(s.into_std()?),
        Either::Right(s) => StdConn::from(s.into_std()?),
    };
    conn.set_nonblocking(false)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::{Hello, Keyed, Runner, Service};

    #[test]
    fn a_runners_debug_never_shows_the_key() {
        let runner = Runner::at("10.0.0.7:7701", Some("secret-key")).unwrap();
        let shown = format!("{runner:?}");
        assert!(shown.contains("10.0.0.7:7701"), "{shown}");
        assert!(!shown.contains("secret-key"), "{shown}");
    }

    #[test]
    fn the_hello_names_its_service_and_carries_the_key() {
        let hello = Hello::Links(Keyed {
            v: 1,
            key: "k".into(),
        });
        assert_eq!(
            serde_json::to_string(&hello).unwrap(),
            r#"{"links":{"v":1,"key":"k"}}"#
        );
        let forwards: Hello = serde_json::from_str(r#"{"forwards":{"v":1,"key":"k"}}"#).unwrap();
        assert!(matches!(forwards, Hello::Forwards(Keyed { v: 1, .. })));
        assert_eq!(
            serde_json::to_string(&Service::Conn(4).hello("k")).unwrap(),
            r#"{"conn":{"v":1,"key":"k","n":4}}"#
        );
    }
}
