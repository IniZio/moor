//! The ledger's connections: the store calls of the Proxy processes, on the ledger socket and,
//! for the shared egress Proxy in another pod, on a TLS listener. A machine runner in another
//! pod reaches the runner socket's loop through that listener too.
//!
//! The ledger stays out of the drain: it accepts and answers until the process exits, so a
//! Proxy keeps its lease and secrets for as long as this controller is alive.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_client::tls::{bridge, RUNNER_HELLO};
use moor_core::config::LedgerTls;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig, ServerConnection, StreamOwned};
use serde::Deserialize;
use zeroize::Zeroizing;

use super::drain::{self, Drain, Handler};
use super::{log, Sink};
use crate::env::{same_key, Env, EGRESS_KEY, MACHINE_SERVICE_KEY};
use crate::vault::Vault;
use crate::Moor;

/// How long a TLS peer gets to finish its handshake and send its hello.
const HELLO_DEADLINE: Duration = Duration::from_secs(10);

/// The longest hello read; anyone who can reach the listener can send one.
const HELLO_MAX: usize = 64 * 1024;

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
    /// The key a machine runner's hello presents; none takes no runner here.
    runner_key: Option<Arc<str>>,
}

impl TlsLedger {
    /// Loads the certificate and key `tls` names, with `~` expanded by `env`, and binds its
    /// address; none when the config sets no `tls`. The listener answers only the shared egress
    /// Proxy and machine runners, so it needs the key of one of them.
    pub(super) fn bind(env: &Env, tls: Option<&LedgerTls>) -> Result<Option<Self>, String> {
        let Some(tls) = tls else { return Ok(None) };
        let runner_key = env.get(MACHINE_SERVICE_KEY).filter(|k| !k.is_empty());
        if env.get(EGRESS_KEY).is_none_or(str::is_empty) && runner_key.is_none() {
            return Err(format!(
                "ledger_tls needs {EGRESS_KEY} or {MACHINE_SERVICE_KEY}: it serves only the \
                 shared egress Proxy and machine runners"
            ));
        }
        let config = server_config(
            &env.expand_home(&tls.cert),
            &env.expand_home(&tls.key),
            &env.expand_home(&tls.client_ca),
        )?;
        let listener = TcpListener::bind(tls.listen)
            .map_err(|e| format!("cannot listen on {} for the ledger: {e}", tls.listen))?;
        Ok(Some(Self {
            listener,
            config: Arc::new(config),
            runner_key: runner_key.map(Arc::from),
        }))
    }

    /// Serves on a thread of its own until the process exits; a runner's connection goes to
    /// `runner` under `drain`, as one on the runner socket does.
    pub(super) fn serve_until_exit(
        self,
        (env, vault): (&Arc<Env>, &Arc<Vault>),
        runner: (Handler, &Arc<Drain>),
        sink: &Sink,
    ) {
        let (env, vault, sink) = (Arc::clone(env), Arc::clone(vault), Arc::clone(sink));
        let (handler, drain) = (runner.0, Arc::clone(runner.1));
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
                        let key = self.runner_key.clone();
                        let runner = (Arc::clone(&handler), Arc::clone(&drain), Arc::clone(&sink));
                        std::thread::spawn(move || {
                            serve_one(stream, (&env, &vault), key.as_deref(), &runner);
                        });
                    }
                    Err(e) => log(&sink, &format!("ledger TLS accept failed: {e}")),
                }
            }
        });
    }
}

/// A runner's hello: `{"runner":{"v":1,"key":<key>}}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnerHello {
    /// What the runner says.
    runner: RunnerSays,
}

/// The body of a runner's hello.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnerSays {
    /// The hello's protocol version.
    v: u32,
    /// The machine service key.
    key: String,
}

/// Serves one connection: a runner's hello hands it to the runner socket's loop, and any other
/// line goes to the ledger as its hello.
fn serve_one(
    mut stream: UntilHello,
    (env, vault): (&Env, &Vault),
    runner_key: Option<&str>,
    runner: &(Handler, Arc<Drain>, Sink),
) {
    let Some(line) = hello_line(&mut stream) else {
        return;
    };
    if let Ok(hello) = serde_json::from_slice::<RunnerHello>(&line) {
        serve_runner(stream, runner_key, &hello.runner, runner);
    } else {
        let head = io::Cursor::new(line);
        Moor::serve_egress_ledger((env, vault), Replayed { head, stream });
    }
}

/// Answers a runner's hello and, when it presents `want`, serves the rest of the connection
/// as a connection on the runner socket.
fn serve_runner(
    mut stream: UntilHello,
    want: Option<&str>,
    hello: &RunnerSays,
    (handler, drain, sink): &(Handler, Arc<Drain>, Sink),
) {
    let verdict = match want {
        _ if hello.v != RUNNER_HELLO => Err(format!(
            "the runner speaks hello {}, the controller speaks {RUNNER_HELLO}",
            hello.v
        )),
        None => Err(format!("{MACHINE_SERVICE_KEY} is unset here")),
        Some(want) if !same_key(want, &hello.key) => Err("not the machine service key".to_owned()),
        Some(_) => Ok(()),
    };
    let answer = match &verdict {
        Ok(()) => serde_json::json!({"Ok": null}),
        Err(why) => serde_json::json!({"Err": {"refused": why}}),
    };
    let answered = writeln!(stream, "{answer}").and_then(|()| stream.flush());
    if answered.is_err() || verdict.is_err() {
        return;
    }
    match bridge(stream.0) {
        Ok(near) => {
            drain::serve_one(near, handler, drain, sink);
        }
        Err(e) => log(sink, &format!("runner connection setup failed: {e}")),
    }
}

/// The first line, newline included, read a byte at a time so nothing after it is taken; at
/// most [`HELLO_MAX`] bytes. None when the peer sends nothing.
fn hello_line(stream: &mut impl Read) -> Option<Zeroizing<Vec<u8>>> {
    let mut line = Zeroizing::new(Vec::new());
    let mut byte = [0_u8; 1];
    while line.len() < HELLO_MAX && line.last() != Some(&b'\n') {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => break,
            Ok(_) => line.extend_from_slice(&byte),
        }
    }
    (!line.is_empty()).then_some(line)
}

/// A connection whose first line was already read, replayed ahead of the rest.
struct Replayed {
    /// The line read.
    head: io::Cursor<Zeroizing<Vec<u8>>>,
    /// The connection after it.
    stream: UntilHello,
}

impl Read for Replayed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.head.read(buf)? {
            0 => self.stream.read(buf),
            n => Ok(n),
        }
    }
}

impl Write for Replayed {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
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

/// A server config presenting the PEM certificate chain at `cert` with the PEM key at `key`,
/// which finishes a handshake only with a client whose certificate chains to the PEM bundle at
/// `client_ca`.
fn server_config(cert: &Path, key: &Path, client_ca: &Path) -> Result<ServerConfig, String> {
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
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let bad_ca =
        |e: &dyn std::fmt::Display| format!("ledger_tls.client_ca {}: {e}", client_ca.display());
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(client_ca).map_err(|e| bad_ca(&e))? {
        roots
            .add(cert.map_err(|e| bad_ca(&e))?)
            .map_err(|e| bad_ca(&e))?;
    }
    if roots.is_empty() {
        return Err(bad_ca(&"holds no certificate"));
    }
    let verifier =
        WebPkiClientVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
            .build()
            .map_err(|e| bad_ca(&e))?;
    ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)
        .map_err(|e| format!("ledger_tls: the key does not fit the certificate: {e}"))
}
