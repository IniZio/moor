//! The ledger's connections: the store calls of the Proxy processes, on the ledger socket and,
//! for the shared egress Proxy in another pod, on a TLS listener.
//!
//! The ledger stays out of the drain: it accepts and answers until the process exits, so a
//! Proxy keeps its lease and secrets for as long as this controller is alive.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::LedgerTls;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

use super::{log, Sink};
use crate::env::{Env, EGRESS_KEY};
use crate::vault::Vault;
use crate::Moor;

/// How long a TLS peer gets to finish its handshake and send its hello.
const HELLO_DEADLINE: Duration = Duration::from_secs(10);

/// Serves `listener` on a thread of its own until the process exits, with `fronts_key` as the
/// front Proxy's key.
pub(super) fn serve_until_exit(
    listener: UnixListener,
    (env, vault): (&Arc<Env>, &Arc<Vault>),
    fronts_key: Option<Arc<str>>,
    sink: &Sink,
) {
    let (env, vault, sink) = (Arc::clone(env), Arc::clone(vault), Arc::clone(sink));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let (env, vault, key) =
                        (Arc::clone(&env), Arc::clone(&vault), fronts_key.clone());
                    std::thread::spawn(move || {
                        Moor::serve_ledger((&env, &vault), key.as_deref(), stream);
                    });
                }
                Err(e) => log(&sink, &format!("ledger accept failed: {e}")),
            }
        }
    });
}

/// The ledger's TCP listener and the certificate it presents.
pub(super) struct TlsLedger {
    /// Bound, not yet accepting.
    pub(super) listener: TcpListener,
    /// The controller's certificate and key.
    config: Arc<ServerConfig>,
}

impl TlsLedger {
    /// Loads the certificate and key `tls` names, with `~` expanded by `env`, and binds its
    /// address; none when the config sets no `tls`. The listener answers only the shared egress
    /// Proxy, so it needs its key.
    pub(super) fn bind(env: &Env, tls: Option<&LedgerTls>) -> Result<Option<Self>, String> {
        let Some(tls) = tls else { return Ok(None) };
        if env.get(EGRESS_KEY).is_none_or(str::is_empty) {
            return Err(format!(
                "ledger_tls needs {EGRESS_KEY}: it serves only the shared egress Proxy"
            ));
        }
        let config = server_config(&env.expand_home(&tls.cert), &env.expand_home(&tls.key))?;
        let listener = TcpListener::bind(tls.listen)
            .map_err(|e| format!("cannot listen on {} for the ledger: {e}", tls.listen))?;
        Ok(Some(Self {
            listener,
            config: Arc::new(config),
        }))
    }

    /// Serves on a thread of its own until the process exits.
    pub(super) fn serve_until_exit(self, (env, vault): (&Arc<Env>, &Arc<Vault>), sink: &Sink) {
        let (env, vault, sink) = (Arc::clone(env), Arc::clone(vault), Arc::clone(sink));
        std::thread::spawn(move || {
            for sock in self.listener.incoming() {
                let started = sock.and_then(|sock| {
                    sock.set_read_timeout(Some(HELLO_DEADLINE))?;
                    sock.set_nodelay(true)?;
                    let conn = ServerConnection::new(Arc::clone(&self.config))
                        .map_err(io::Error::other)?;
                    Ok(UntilHello(StreamOwned::new(conn, sock)))
                });
                match started {
                    Ok(stream) => {
                        let (env, vault) = (Arc::clone(&env), Arc::clone(&vault));
                        std::thread::spawn(move || {
                            Moor::serve_egress_ledger((&env, &vault), stream);
                        });
                    }
                    Err(e) => log(&sink, &format!("ledger TLS accept failed: {e}")),
                }
            }
        });
    }
}

/// A TLS connection whose read deadline covers only the handshake and the hello. The
/// controller's first write answers the hello, so it lifts the deadline, and an idle pooled
/// connection then waits as long as the Proxy keeps it.
struct UntilHello(StreamOwned<ServerConnection, TcpStream>);

impl Read for UntilHello {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for UntilHello {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.0.sock.read_timeout()?.is_some() {
            self.0.sock.set_read_timeout(None)?;
        }
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// A server config presenting the PEM certificate chain at `cert` with the PEM key at `key`.
fn server_config(cert: &Path, key: &Path) -> Result<ServerConfig, String> {
    let chain = CertificateDer::pem_file_iter(cert)
        .and_then(Iterator::collect::<Result<Vec<_>, _>>)
        .map_err(|e| format!("ledger_tls.cert {}: {e}", cert.display()))?;
    if chain.is_empty() {
        return Err(format!(
            "ledger_tls.cert {} holds no certificate",
            cert.display()
        ));
    }
    let key = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| format!("ledger_tls.key {}: {e}", key.display()))?;
    ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|e| format!("ledger_tls: the key does not fit the certificate: {e}"))
}
