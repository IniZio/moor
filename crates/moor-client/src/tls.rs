//! The dial to the controller's TLS listener from a machine runner or a front in another pod, and
//! the pump that makes a TLS stream look like a Unix socket to the code on either end.
//!
//! A runner opens with `{"runner":{"v":1,"key":<key>}}` and a front with
//! `{"front":{"v":1,"ns":<namespace>,"key":<key>}}`. The controller answers `{"Ok":null}` or
//! `{"Err":{"refused":<why>}}`. From then on the stream carries the controller socket's protocol.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::ops::DerefMut;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustix::event::{poll, PollFd, PollFlags};
use rustix::io::Errno;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::StreamOwned;
use rustls::{ClientConfig, ClientConnection, ConnectionCommon, RootCertStore, SideData};

use super::remote::dial;
use super::{Attempt, Client, ClientError};

/// The runner hello's protocol version.
pub const RUNNER_HELLO: u32 = 1;

/// The front hello's protocol version.
pub const FRONT_HELLO: u32 = 1;

/// The longest hello answer a dialer reads.
const ANSWER_MAX: usize = 4096;

/// The PEM files of a TLS dial to the controller.
#[derive(Debug, Clone)]
pub struct TlsPki {
    /// The CA that signed the controller's certificate.
    pub ca: PathBuf,
    /// The certificate the dialer presents.
    pub cert: PathBuf,
    /// The certificate's private key.
    pub key: PathBuf,
}

/// The controller's TLS listener, as a runner or a front dials it.
#[derive(Clone)]
pub(super) struct Tls {
    /// Host to dial and to check the certificate against.
    host: String,
    /// Port of the listener.
    port: u16,
    /// Trusts only the controller's CA.
    config: Arc<ClientConfig>,
    /// The hello line, which carries the key.
    hello: Arc<str>,
}

impl fmt::Debug for Tls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tls(tls://{}:{})", self.host, self.port)
    }
}

impl Client {
    /// A client for the controller's TLS listener at `controller`, `tls://HOST:PORT`, whose
    /// certificate `pki.ca` signed. Each connection presents `pki.cert` and opens with the
    /// runner hello and `key`.
    ///
    /// # Errors
    /// The address is not `tls://HOST:PORT`, or a PEM file cannot be read or does not fit.
    pub fn runner(controller: &str, pki: &TlsPki, key: &str) -> Result<Self, String> {
        let hello = serde_json::json!({"runner": {"v": RUNNER_HELLO, "key": key}});
        Self::over_tls(controller, pki, &hello)
    }

    /// A client for the controller's TLS listener at `controller`, `tls://HOST:PORT`, whose
    /// certificate `pki.ca` signed. Each connection presents `pki.cert` and opens with the
    /// front hello for namespace `ns` and `key`, and then speaks as that front.
    ///
    /// # Errors
    /// The address is not `tls://HOST:PORT`, or a PEM file cannot be read or does not fit.
    pub fn front(controller: &str, pki: &TlsPki, ns: &str, key: &str) -> Result<Self, String> {
        let hello = serde_json::json!({"front": {"v": FRONT_HELLO, "ns": ns, "key": key}});
        Self::over_tls(controller, pki, &hello)
    }

    /// A client whose connections each open with `hello`.
    fn over_tls(controller: &str, pki: &TlsPki, hello: &serde_json::Value) -> Result<Self, String> {
        let rest = controller
            .strip_prefix("tls://")
            .ok_or_else(|| format!("{controller}: expected tls://HOST:PORT"))?;
        let (host, port) = rest
            .rsplit_once(':')
            .and_then(|(h, p)| Some((h, p.parse::<u16>().ok()?)))
            .ok_or_else(|| format!("{controller}: expected tls://HOST:PORT"))?;
        let host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        Ok(Self::linked(super::remote::Link::Tls(Tls {
            host,
            port,
            config: Arc::new(client_config(pki)?),
            hello: Arc::from(hello.to_string()),
        })))
    }
}

impl Tls {
    /// Dials, says the hello and returns the client's end of the pumped connection. Nothing
    /// of the protocol has been sent yet, so any failure leaves nothing run.
    pub(super) fn connect(&self) -> Attempt<UnixStream, ClientError> {
        let tcp = match dial(&self.host, self.port) {
            Ok(tcp) => tcp,
            Err(why) => return Attempt::Unrun(ClientError::Remote(why)),
        };
        match self.hello(tcp) {
            Ok(tls) => match bridge(tls) {
                Ok(near) => Attempt::Done(near),
                Err(e) => Attempt::Failed(e.into()),
            },
            Err(e) => e,
        }
    }

    /// Completes TLS and the hello on `tcp`.
    fn hello(
        &self,
        tcp: TcpStream,
    ) -> Result<StreamOwned<ClientConnection, TcpStream>, Attempt<UnixStream, ClientError>> {
        let bad = |why: String| Attempt::Failed(ClientError::Remote(why));
        let name = ServerName::try_from(self.host.clone())
            .map_err(|_| bad(format!("{} is not a valid host name", self.host)))?;
        let conn = ClientConnection::new(Arc::clone(&self.config), name)
            .map_err(|e| bad(e.to_string()))?;
        let mut tls = StreamOwned::new(conn, tcp);
        writeln!(tls, "{}", self.hello)
            .and_then(|()| tls.flush())
            .map_err(|e| down(&e))?;
        let answer = line(&mut tls).map_err(|e| down(&e))?;
        let answer: serde_json::Value = serde_json::from_slice(&answer)
            .map_err(|e| bad(format!("the controller's hello answer: {e}")))?;
        if answer.get("Ok").is_some() {
            return Ok(tls);
        }
        let why = answer
            .pointer("/Err/refused")
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| answer.to_string(), str::to_owned);
        Err(Attempt::Failed(ClientError::Unauthorized(why)))
    }
}

/// A hello that failed with `e`: a TLS error, such as a refused certificate, fails the same
/// way on every dial, while any other may pass on the next.
fn down(e: &io::Error) -> Attempt<UnixStream, ClientError> {
    e.get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        .map_or_else(
            || Attempt::Unrun(ClientError::Remote(format!("controller: {e}"))),
            |tls| Attempt::Failed(ClientError::Unauthorized(format!("controller TLS: {tls}"))),
        )
}

/// One line read a byte at a time, so nothing after it is taken from the stream.
fn line(read: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    while line.len() < ANSWER_MAX {
        if read.read(&mut byte)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if &byte == b"\n" {
            return Ok(line);
        }
        line.extend_from_slice(&byte);
    }
    Err(io::Error::other("the hello answer is too long"))
}

/// TLS that trusts only the certificates in `pki.ca` and presents `pki.cert`.
fn client_config(pki: &TlsPki) -> Result<ClientConfig, String> {
    fn named(path: &Path) -> impl Fn(rustls::pki_types::pem::Error) -> String + '_ {
        move |e| format!("{}: {e}", path.display())
    }
    let chain = CertificateDer::pem_file_iter(&pki.cert)
        .and_then(Iterator::collect::<Result<Vec<_>, _>>)
        .map_err(named(&pki.cert))?;
    if chain.is_empty() {
        return Err(format!("{} holds no certificate", pki.cert.display()));
    }
    let key = PrivateKeyDer::from_pem_file(&pki.key).map_err(named(&pki.key))?;
    trusting(&pki.ca)?
        .with_client_auth_cert(chain, key)
        .map_err(|e| {
            format!(
                "{}: the key does not fit the certificate: {e}",
                pki.key.display()
            )
        })
}

/// TLS that trusts only the certificates in the PEM file `ca`, before its client certificate.
fn trusting(
    ca: &Path,
) -> Result<rustls::ConfigBuilder<ClientConfig, rustls::client::WantsClientCert>, String> {
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(ca).map_err(|e| format!("{}: {e}", ca.display()))? {
        roots
            .add(cert.map_err(|e| format!("{}: {e}", ca.display()))?)
            .map_err(|e| format!("{}: {e}", ca.display()))?;
    }
    if roots.is_empty() {
        return Err(format!("{} holds no certificate", ca.display()));
    }
    let builder =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots);
    Ok(builder)
}

/// Joins `tls`, past its hello, to a fresh socket pair and returns the near end; a thread
/// pumps bytes both ways. Each side's end of writing reaches the other as a half-close.
///
/// # Errors
/// The socket pair or the thread cannot be made.
pub fn bridge<C, S>(tls: StreamOwned<C, TcpStream>) -> io::Result<UnixStream>
where
    C: DerefMut<Target = ConnectionCommon<S>> + Send + 'static,
    S: SideData + 'static,
{
    let (near, far) = UnixStream::pair()?;
    tls.sock.set_read_timeout(None)?;
    tls.sock.set_write_timeout(None)?;
    std::thread::Builder::new()
        .name("moor-tls".to_owned())
        .spawn(move || {
            let mut tls = tls;
            let _ = pump(&mut tls, &far);
            let _ = far.shutdown(Shutdown::Both);
            let _ = tls.sock.shutdown(Shutdown::Both);
        })?;
    Ok(near)
}

/// Moves bytes until both sides have ended their writes or either fails.
fn pump<C, S>(tls: &mut StreamOwned<C, TcpStream>, mut far: &UnixStream) -> io::Result<()>
where
    C: DerefMut<Target = ConnectionCommon<S>>,
    S: SideData,
{
    let mut buf = vec![0_u8; 16 * 1024];
    let (mut from_peer, mut from_near) = (true, true);
    loop {
        while tls.conn.wants_write() {
            tls.conn.write_tls(&mut tls.sock)?;
        }
        if !from_peer && !from_near {
            return Ok(());
        }
        let (peer_ready, near_ready) = ready(&tls.sock, far, from_peer, from_near)?;
        if peer_ready && tls.conn.read_tls(&mut tls.sock)? == 0 {
            from_peer = false;
            far.shutdown(Shutdown::Write)?;
        } else if peer_ready {
            tls.conn.process_new_packets().map_err(io::Error::other)?;
            loop {
                match tls.conn.reader().read(&mut buf) {
                    Ok(0) => {
                        from_peer = false;
                        far.shutdown(Shutdown::Write)?;
                        break;
                    }
                    Ok(n) => far.write_all(buf.get(..n).unwrap_or_default())?,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e),
                }
            }
        }
        if near_ready {
            match far.read(&mut buf)? {
                0 => {
                    from_near = false;
                    tls.conn.send_close_notify();
                }
                n => tls
                    .conn
                    .writer()
                    .write_all(buf.get(..n).unwrap_or_default())?,
            }
        }
    }
}

/// Waits until the peer's socket or the near end has something to read, of those still open.
fn ready(
    peer: &TcpStream,
    far: &UnixStream,
    on_peer: bool,
    on_near: bool,
) -> io::Result<(bool, bool)> {
    let flags = |on: bool| {
        if on {
            PollFlags::IN
        } else {
            PollFlags::empty()
        }
    };
    let mut fds = [
        PollFd::new(peer, flags(on_peer)),
        PollFd::new(far, flags(on_near)),
    ];
    loop {
        match poll(&mut fds, None) {
            Ok(_) => break,
            Err(Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
    }
    let up = |fd: &PollFd<'_>, on: bool| on && !fd.revents().is_empty();
    let [peer, far] = &fds;
    Ok((up(peer, on_peer), up(far, on_near)))
}

#[cfg(test)]
mod tests;
