//! Where a Proxy reaches the ledger: a controller's unix socket on this host, or its TLS
//! listener in another pod.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
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

/// The PEM files a Proxy dials a TLS ledger with.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TlsFiles<'a> {
    /// The CA bundle the controller's certificate must chain to.
    pub(crate) ca: &'a Path,
    /// The client certificate the Proxy presents, which the ledger CA signed.
    pub(crate) cert: &'a Path,
    /// The private key of `cert`.
    pub(crate) key: &'a Path,
}

/// A TLS ledger address and the client config that checks it.
#[derive(Clone)]
pub(crate) struct Tls {
    /// The host to dial, and the name its certificate must carry.
    host: ServerName<'static>,
    /// The port to dial.
    port: u16,
    /// Trusts only the CA bundle it was given, and presents the client certificate.
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
    /// `ledger`, a unix socket path or `tls://host:port`. A TLS address needs `tls`, and a
    /// socket path takes none.
    pub(crate) fn parse(ledger: &str, tls: Option<TlsFiles<'_>>) -> Result<Self, String> {
        let Some(addr) = ledger.strip_prefix(TLS) else {
            if tls.is_some() {
                return Err(
                    "a CA bundle and client certificate only apply to a tls:// ledger".to_owned(),
                );
            }
            return Ok(Self::Unix(ledger.into()));
        };
        let tls = tls.ok_or(
            "a tls:// ledger needs a CA bundle to check the controller and a client certificate to present",
        )?;
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
            config: Arc::new(client_config(tls)?),
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

/// A client config that trusts only the certificates in `tls.ca` and presents `tls.cert`.
fn client_config(tls: TlsFiles<'_>) -> Result<ClientConfig, String> {
    let TlsFiles { ca, cert, key } = tls;
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
    let chain = CertificateDer::pem_file_iter(cert)
        .and_then(Iterator::collect::<Result<Vec<_>, _>>)
        .map_err(|e| format!("client certificate {}: {e}", cert.display()))?;
    if chain.is_empty() {
        return Err(format!(
            "client certificate {} holds no certificate",
            cert.display()
        ));
    }
    let key = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| format!("client key {}: {e}", key.display()))?;
    ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| bad(&e))?
        .with_root_certificates(roots)
        .with_client_auth_cert(chain, key)
        .map_err(|e| format!("the client key does not fit its certificate: {e}"))
}
