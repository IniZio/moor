//! Where a Proxy reaches the ledger: a controller's unix socket on this host, or its TLS
//! listener in another pod.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

/// The scheme of a TLS ledger address.
const TLS: &str = "tls://";

/// Where the ledger is.
#[derive(Clone, Debug)]
pub(crate) enum Endpoint {
    /// The ledger socket of a controller on this host.
    Unix(PathBuf),
    /// A controller's TLS listener, whose certificate must chain to the given CA bundle.
    Tls(Tls),
}

/// A TLS ledger address and the client config that checks it.
#[derive(Clone)]
pub(crate) struct Tls {
    /// The host to dial, and the name its certificate must carry.
    host: ServerName<'static>,
    /// The port to dial.
    port: u16,
    /// Trusts only the CA bundle it was given.
    config: Arc<ClientConfig>,
}

impl std::fmt::Debug for Tls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tls://{}:{}", self.host.to_str(), self.port)
    }
}

/// A connection to the ledger, whichever way it was dialled.
pub(crate) trait Duplex: Read + Write + Send {}

impl<T: Read + Write + Send> Duplex for T {}

impl Endpoint {
    /// `ledger`, a unix socket path or `tls://host:port`. `ca` is the PEM bundle that checks a
    /// TLS controller's certificate; a TLS address needs it and a socket path takes none.
    pub(crate) fn parse(ledger: &str, ca: Option<&Path>) -> Result<Self, String> {
        let Some(addr) = ledger.strip_prefix(TLS) else {
            if ca.is_some() {
                return Err("a CA bundle only applies to a tls:// ledger".to_owned());
            }
            return Ok(Self::Unix(ledger.into()));
        };
        let ca = ca.ok_or("a tls:// ledger needs a CA bundle to check the controller")?;
        let (host, port) = addr
            .rsplit_once(':')
            .ok_or_else(|| format!("{ledger}: no port"))?;
        let port = port
            .parse()
            .map_err(|_| format!("{ledger}: bad port {port}"))?;
        let host = ServerName::try_from(host.trim_start_matches('[').trim_end_matches(']'))
            .map_err(|e| format!("{ledger}: bad host: {e}"))?
            .to_owned();
        Ok(Self::Tls(Tls {
            host,
            port,
            config: Arc::new(client_config(ca)?),
        }))
    }

    /// A new connection, its TLS handshake done, with `timeout` on connecting and on each read
    /// and write.
    pub(crate) fn dial(&self, timeout: Duration) -> io::Result<Box<dyn Duplex>> {
        match self {
            Self::Unix(socket) => {
                let stream = UnixStream::connect(socket)?;
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))?;
                Ok(Box::new(stream))
            }
            Self::Tls(tls) => tls.dial(timeout).map(|s| Box::new(s) as Box<dyn Duplex>),
        }
    }
}

impl Tls {
    /// A TLS connection past its handshake, so a certificate the CA did not sign fails here.
    fn dial(&self, timeout: Duration) -> io::Result<StreamOwned<ClientConnection, TcpStream>> {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "the host has no address");
        for addr in (self.host.to_str().as_ref(), self.port).to_socket_addrs()? {
            match TcpStream::connect_timeout(&addr, timeout) {
                Ok(sock) => {
                    sock.set_read_timeout(Some(timeout))?;
                    sock.set_write_timeout(Some(timeout))?;
                    sock.set_nodelay(true)?;
                    let conn = ClientConnection::new(Arc::clone(&self.config), self.host.clone())
                        .map_err(io::Error::other)?;
                    let mut tls = StreamOwned::new(conn, sock);
                    while tls.conn.is_handshaking() {
                        tls.conn.complete_io(&mut tls.sock)?;
                    }
                    return Ok(tls);
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

/// A client config that trusts only the certificates in the PEM file `ca`.
fn client_config(ca: &Path) -> Result<ClientConfig, String> {
    let bad = |e: &dyn std::fmt::Display| format!("CA bundle {}: {e}", ca.display());
    let certs = CertificateDer::pem_file_iter(ca)
        .map_err(|e| bad(&e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| bad(&e))?;
    let mut roots = RootCertStore::empty();
    let (added, _) = roots.add_parsable_certificates(certs);
    if added == 0 {
        return Err(bad(&"holds no certificate"));
    }
    ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| bad(&e))
        .map(|b| b.with_root_certificates(roots).with_no_client_auth())
}
