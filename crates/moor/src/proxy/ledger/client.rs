//! The Proxy's end of the ledger: a small pool of connections to the controller.

use std::io::{self, BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use moor_core::user::{SecretName, UserId};
use moor_core::BoxId;
use moor_core::Secret;
use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

use super::{Call, Fault, Hello, LedgerError, LEDGER};
use crate::ledger_dir;

/// How long one call may wait on the controller before it counts as unavailable.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Connections to the ledger socket, each opened with the same hello.
pub(super) struct Ledger {
    /// The ledger socket.
    socket: PathBuf,
    /// The hello line, which carries the key.
    hello: Zeroizing<String>,
    /// Connections between calls.
    idle: Mutex<Vec<Conn>>,
}

/// One connection past its hello.
struct Conn {
    /// Reads replies.
    read: BufReader<UnixStream>,
    /// Sends calls.
    write: UnixStream,
}

impl Ledger {
    /// A pool for `socket` that dials on first use.
    pub(super) fn new(socket: PathBuf, hello: &Hello) -> Self {
        let hello = Zeroizing::new(serde_json::to_string(hello).unwrap_or_default());
        Self {
            socket,
            hello,
            idle: Mutex::new(Vec::new()),
        }
    }

    /// Makes `call` and decodes its value.
    ///
    /// An idle connection that fails means the controller closed it, so every idle one goes. A
    /// [`Call::repeatable`] call is then made again on a fresh connection; any other fails, since
    /// the controller may have applied it before it went away.
    pub(super) fn call<T: DeserializeOwned>(&self, call: &Call) -> Result<T, LedgerError> {
        let line = Zeroizing::new(
            serde_json::to_string(call).map_err(|e| LedgerError::Refused(e.to_string()))?,
        );
        let pooled = self
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        if let Some(mut conn) = pooled {
            match conn.round::<T>(&line) {
                Ok(reply) => {
                    self.put(conn);
                    return reply.map_err(LedgerError::from);
                }
                Err(e) => {
                    self.idle
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clear();
                    if !call.repeatable() {
                        return Err(unavailable(&e));
                    }
                }
            }
        }
        let mut conn = self.dial()?;
        let reply = conn.round::<T>(&line).map_err(|e| unavailable(&e))?;
        self.put(conn);
        reply.map_err(LedgerError::from)
    }

    /// Keeps `conn` for the next call.
    fn put(&self, conn: Conn) {
        self.idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(conn);
    }

    /// A new connection past its hello.
    fn dial(&self) -> Result<Conn, LedgerError> {
        let stream = UnixStream::connect(&self.socket).map_err(|e| unavailable(&e))?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
            .map_err(|e| unavailable(&e))?;
        let read = BufReader::new(stream.try_clone().map_err(|e| unavailable(&e))?);
        let mut conn = Conn {
            read,
            write: stream,
        };
        conn.round::<()>(&self.hello)
            .map_err(|e| unavailable(&e))?
            .map_err(LedgerError::from)?;
        Ok(conn)
    }
}

impl Conn {
    /// Sends `line` and reads its reply; `Err` means the connection is no good.
    fn round<T: DeserializeOwned>(&mut self, line: &str) -> io::Result<Result<T, Fault>> {
        self.write.write_all(line.as_bytes())?;
        self.write.write_all(b"\n")?;
        self.write.flush()?;
        let mut reply = Zeroizing::new(String::new());
        if self.read.read_line(&mut reply)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        serde_json::from_str(&reply).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// The error for a controller that cannot be reached.
fn unavailable(e: &io::Error) -> LedgerError {
    LedgerError::Unavailable(e.to_string())
}

/// The front Proxy's ledger.
pub(crate) struct FrontsLedger(Ledger);

impl FrontsLedger {
    /// The ledger of the controller whose state dir is `state_dir`, presenting `key`.
    pub(crate) fn new(state_dir: &Path, key: String) -> Self {
        Self::at(ledger_dir::socket(state_dir), key)
    }

    /// The ledger on `socket`, presenting `key`.
    pub(crate) fn at(socket: PathBuf, key: String) -> Self {
        Self(Ledger::new(socket, &Hello::Fronts { v: LEDGER, key }))
    }

    /// The user the live lease `token` of `front` acts for.
    pub(crate) fn lease_user(
        &self,
        token: &[u8],
        front: &str,
    ) -> Result<Option<UserId>, LedgerError> {
        self.0.call(&Call::FrontLeaseUser {
            token: token.to_vec(),
            front: front.to_owned(),
        })
    }

    /// `user`'s secret `name`, while they hold a live lease of `front`.
    pub(crate) fn secret(
        &self,
        front: &str,
        user: UserId,
        name: &SecretName,
    ) -> Result<Option<Secret>, LedgerError> {
        let bytes: Option<Vec<u8>> = self.0.call(&Call::FrontSecret {
            front: front.to_owned(),
            user,
            name: name.clone(),
        })?;
        Ok(bytes.map(Secret::new))
    }
}

/// The shared egress Proxy's token lookups.
pub(crate) struct EgressLedger(Ledger);

impl EgressLedger {
    /// The ledger on `socket`, presenting `key`.
    pub(crate) fn at(socket: PathBuf, key: String) -> Self {
        let hello = Hello::Egress {
            v: LEDGER,
            key,
            box_id: None,
        };
        Self(Ledger::new(socket, &hello))
    }

    /// The open Box whose egress token hashes to `token_hash`.
    pub(crate) fn token_box(&self, token_hash: &str) -> Result<Option<BoxId>, LedgerError> {
        self.0.call(&Call::TokenBox {
            token_hash: token_hash.to_owned(),
        })
    }
}
