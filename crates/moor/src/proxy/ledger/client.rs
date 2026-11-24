//! The Proxy's end of the ledger: a small pool of connections to the controller.

use std::io::{self, BufRead as _, BufReader, Write as _};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use moor_core::BoxId;
use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

use super::endpoint::Duplex;
use super::{Call, Endpoint, Fault, Hello, LedgerError, Owner, LEDGER};

/// How long one call may wait on the controller before it counts as unavailable.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Connections to the ledger, each opened with the same hello.
pub(super) struct Ledger {
    /// Where the ledger is.
    endpoint: Endpoint,
    /// The hello line, which carries the key.
    hello: Zeroizing<String>,
    /// Connections between calls.
    idle: Mutex<Vec<Conn>>,
}

/// One connection past its hello; calls are written through the reader.
struct Conn(BufReader<Box<dyn Duplex>>);

impl Ledger {
    /// A pool for `endpoint` that dials on first use.
    pub(super) fn new(endpoint: Endpoint, hello: &Hello) -> Self {
        let hello = Zeroizing::new(serde_json::to_string(hello).unwrap_or_default());
        Self {
            endpoint,
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
        let stream = self.endpoint.dial(TIMEOUT).map_err(|e| unavailable(&e))?;
        let mut conn = Conn(BufReader::new(stream));
        conn.round::<()>(&self.hello)
            .map_err(|e| unavailable(&e))?
            .map_err(LedgerError::from)?;
        Ok(conn)
    }
}

impl Conn {
    /// Sends `line` and reads its reply; `Err` means the connection is no good.
    fn round<T: DeserializeOwned>(&mut self, line: &str) -> io::Result<Result<T, Fault>> {
        let write = self.0.get_mut();
        write.write_all(line.as_bytes())?;
        write.write_all(b"\n")?;
        write.flush()?;
        let mut reply = Zeroizing::new(String::new());
        if self.0.read_line(&mut reply)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        serde_json::from_str(&reply).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// The error for a controller that cannot be reached.
fn unavailable(e: &io::Error) -> LedgerError {
    LedgerError::Unavailable(e.to_string())
}

/// The shared egress Proxy's token lookups.
pub(crate) struct EgressLedger(Ledger);

impl EgressLedger {
    /// The ledger at `endpoint`, presenting `key`.
    pub(crate) fn at(endpoint: Endpoint, key: String) -> Self {
        let hello = Hello::Egress {
            v: LEDGER,
            key,
            box_id: None,
        };
        Self(Ledger::new(endpoint, &hello))
    }

    /// The token hash and Box of every open, awake Box that has an egress token.
    pub(crate) fn awake_tokens(&self) -> Result<super::AwakeTokens, LedgerError> {
        self.0.call(&Call::AwakeTokens)
    }

    /// The open Box whose egress token hashes to `token_hash`, with a digest of its spec. A
    /// controller that refuses the call is asked [`Self::token_box`] instead, with no digest.
    pub(crate) fn token_spec(&self, token_hash: &str) -> Result<Option<Owner>, LedgerError> {
        let call = Call::TokenSpec {
            token_hash: token_hash.to_owned(),
        };
        match self.0.call(&call) {
            Err(LedgerError::Refused(_)) => Ok(self
                .token_box(token_hash)?
                .map(|box_id| Owner { box_id, spec: None })),
            answer => answer,
        }
    }

    /// The open Box whose egress token hashes to `token_hash`.
    pub(crate) fn token_box(&self, token_hash: &str) -> Result<Option<BoxId>, LedgerError> {
        self.0.call(&Call::TokenBox {
            token_hash: token_hash.to_owned(),
        })
    }
}
