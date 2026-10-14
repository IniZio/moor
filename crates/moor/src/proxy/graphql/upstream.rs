//! TLS client connections from the GraphQL filter to the real upstream.

use std::fmt;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use crate::proxy::guard::{Guard, GuardError};

/// How long a TCP connect may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a single read or write may block.
const IO_TIMEOUT: Duration = Duration::from_secs(60);

/// A completed TLS session over TCP to the upstream.
pub(crate) type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// Why an upstream client could not be built or connected.
#[derive(Debug, thiserror::Error)]
pub(crate) enum UpstreamError {
    /// The extra root file could not be read or held no usable certificate.
    #[error("cannot load extra root certificates from {path}: {reason}")]
    Roots {
        /// File that failed to load.
        path: String,
        /// What went wrong.
        reason: String,
    },
    /// The host is not a valid TLS server name.
    #[error("invalid upstream host name {0:?}")]
    Host(String),
    /// TLS client settings were rejected.
    #[error("cannot configure TLS client: {0}")]
    Config(#[source] rustls::Error),
    /// The destination guard refused the host.
    #[error("upstream destination refused: {0}")]
    Guard(#[source] GuardError),
    /// No address accepted a TCP connection.
    #[error("cannot connect to upstream {addr}: {source}")]
    Connect {
        /// Address that was tried last.
        addr: String,
        /// Underlying socket error.
        #[source]
        source: std::io::Error,
    },
    /// The TLS handshake failed.
    #[error("TLS handshake with upstream failed: {0}")]
    Handshake(#[source] std::io::Error),
}

/// Dials one upstream host over TLS with a fixed trust store.
pub(crate) struct Upstream {
    /// Host name used for DNS, SNI and certificate verification.
    host: String,
    /// TCP port.
    port: u16,
    /// Address to dial instead of resolving `host`.
    dial: Option<SocketAddr>,
    /// Vets resolved addresses before any dial.
    guard: Guard,
    /// Shared TLS client settings.
    config: Arc<ClientConfig>,
}

impl fmt::Debug for Upstream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Upstream")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("dial", &self.dial)
            .finish_non_exhaustive()
    }
}

/// Reads every certificate from the PEM file at `path`.
fn load_pem(path: &Path) -> Result<Vec<CertificateDer<'static>>, UpstreamError> {
    let fail = |reason: String| UpstreamError::Roots {
        path: path.display().to_string(),
        reason,
    };
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(|e| fail(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| fail(e.to_string()))?;
    if certs.is_empty() {
        return Err(fail("no certificates found".to_owned()));
    }
    Ok(certs)
}

impl Upstream {
    /// Builds a client for `host:port`.
    ///
    /// `dial` overrides DNS; `extra_root` is a PEM file of additional trusted roots.
    pub(crate) fn new(
        host: &str,
        port: u16,
        dial: Option<SocketAddr>,
        extra_root: Option<&Path>,
    ) -> Result<Self, UpstreamError> {
        ServerName::try_from(host.to_owned()).map_err(|_| UpstreamError::Host(host.to_owned()))?;
        let mut roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        if let Some(path) = extra_root {
            for cert in load_pem(path)? {
                roots.add(cert).map_err(|e| UpstreamError::Roots {
                    path: path.display().to_string(),
                    reason: e.to_string(),
                })?;
            }
        }
        let mut config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(UpstreamError::Config)?
                .with_root_certificates(roots)
                .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            host: host.to_owned(),
            port,
            dial,
            guard: Guard::default(),
            config: Arc::new(config),
        })
    }

    /// Replaces the destination guard.
    pub(crate) fn with_guard(mut self, guard: Guard) -> Self {
        self.guard = guard;
        self
    }

    /// Host name used for SNI and certificate verification.
    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    /// Opens TCP to the upstream and completes the TLS handshake.
    pub(crate) fn connect(&self) -> Result<TlsStream, UpstreamError> {
        let name = ServerName::try_from(self.host.clone())
            .map_err(|_| UpstreamError::Host(self.host.clone()))?;
        let mut tcp = self.dial_tcp()?;
        tcp.set_read_timeout(Some(IO_TIMEOUT))
            .and_then(|()| tcp.set_write_timeout(Some(IO_TIMEOUT)))
            .map_err(UpstreamError::Handshake)?;
        let mut conn =
            ClientConnection::new(Arc::clone(&self.config), name).map_err(UpstreamError::Config)?;
        while conn.is_handshaking() {
            conn.complete_io(&mut tcp)
                .map_err(UpstreamError::Handshake)?;
        }
        Ok(StreamOwned::new(conn, tcp))
    }

    /// Opens a TCP connection to the dial override or the resolved host.
    fn dial_tcp(&self) -> Result<TcpStream, UpstreamError> {
        let addrs: Vec<SocketAddr> = match self.dial {
            Some(addr) => vec![addr],
            None => self
                .guard
                .resolve(&self.host, self.port)
                .map_err(UpstreamError::Guard)?,
        };
        let mut last = None;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(tcp) => return Ok(tcp),
                Err(source) => {
                    last = Some(UpstreamError::Connect {
                        addr: addr.to_string(),
                        source,
                    });
                }
            }
        }
        Err(last.unwrap_or_else(|| {
            UpstreamError::Guard(GuardError::Unresolved {
                host: self.host.clone(),
                port: self.port,
            })
        }))
    }
}

#[cfg(test)]
mod tests;
