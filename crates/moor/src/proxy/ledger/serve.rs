//! The controller's end of the ledger: one connection, its hello, then its calls.

use std::io::{BufRead, Write};

use moor_core::user::{SecretName, UserId};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use super::{Call, Fault, Hello, LEDGER};
use crate::store::Store;
use crate::vault::Vault;

/// The keys the controller started its Proxy processes with.
#[derive(Debug, Default)]
pub(crate) struct Keys<'a> {
    /// The front Proxy's key; none when no front Proxy runs.
    pub(crate) fronts: Option<&'a str>,
}

/// Whom a connection's hello proved it speaks for.
enum Bound {
    /// The front Proxy.
    Fronts,
}

/// Serves one connection until EOF or a failed write.
///
/// Each call runs only while `answering` gives a guard, which a draining controller withholds.
pub(crate) fn serve<G>(
    (store, vault): (&Store, &Vault),
    keys: &Keys<'_>,
    mut read: impl BufRead,
    mut write: impl Write,
    mut answering: impl FnMut() -> Option<G>,
) {
    let Some(line) = next(&mut read) else { return };
    let bound = match hello(keys, &line) {
        Ok(bound) => bound,
        Err(fault) => {
            send(&mut write, &encode(&Err::<(), _>(fault)));
            return;
        }
    };
    if !send(&mut write, &encode(&Ok::<(), Fault>(()))) {
        return;
    }
    while let Some(line) = next(&mut read) {
        let Some(_held) = answering() else { return };
        if !send(&mut write, &answer((store, vault), &bound, &line)) {
            return;
        }
    }
}

/// The next line, or `None` at EOF or on a read error.
fn next(read: &mut impl BufRead) -> Option<Zeroizing<String>> {
    let mut line = Zeroizing::new(String::new());
    match read.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

/// Writes `line` and a newline; false when the peer is gone.
fn send(write: &mut impl Write, line: &str) -> bool {
    write
        .write_all(line.as_bytes())
        .and_then(|()| write.write_all(b"\n"))
        .and_then(|()| write.flush())
        .is_ok()
}

/// `reply` as one line.
fn encode<T: Serialize>(reply: &Result<T, Fault>) -> Zeroizing<String> {
    Zeroizing::new(
        serde_json::to_string(reply)
            .unwrap_or_else(|e| format!(r#"{{"Err":{{"store":"cannot encode the reply: {e}"}}}}"#)),
    )
}

/// Whom `line`, a hello, speaks for.
fn hello(keys: &Keys<'_>, line: &str) -> Result<Bound, Fault> {
    let hello: Hello =
        serde_json::from_str(line).map_err(|e| Fault::Refused(format!("bad hello: {e}")))?;
    match hello {
        Hello::Fronts { v, key } => {
            speaks(v)?;
            let want = keys
                .fronts
                .ok_or_else(|| Fault::Refused("no front Proxy runs here".to_owned()))?;
            if !same(want, &key) {
                return Err(Fault::Refused("not the front Proxy's key".to_owned()));
            }
            Ok(Bound::Fronts)
        }
    }
}

/// Refuses a protocol other than this build's.
fn speaks(v: u32) -> Result<(), Fault> {
    if v == LEDGER {
        return Ok(());
    }
    Err(Fault::Refused(format!(
        "the Proxy speaks ledger {v}, the controller speaks {LEDGER}"
    )))
}

/// Whether two keys match; their digests are compared, so timing says nothing of the key.
fn same(want: &str, got: &str) -> bool {
    Sha256::digest(want.as_bytes()) == Sha256::digest(got.as_bytes())
}

/// The reply to `line`, a call by `bound`.
fn answer((store, vault): (&Store, &Vault), bound: &Bound, line: &str) -> Zeroizing<String> {
    let call: Call = match serde_json::from_str(line) {
        Ok(call) => call,
        Err(e) => return encode(&Err::<(), _>(Fault::Refused(format!("bad call: {e}")))),
    };
    match (bound, call) {
        (Bound::Fronts, Call::FrontLeaseUser { token, front }) => {
            encode(&store.front_lease_user(&token, &front).map_err(Fault::from))
        }
        (Bound::Fronts, Call::FrontSecret { front, user, name }) => {
            encode(&front_secret((store, vault), &front, user, &name))
        }
    }
}

/// `user`'s secret `name` as bytes, refused unless they hold a live lease of `front`.
fn front_secret(
    (store, vault): (&Store, &Vault),
    front: &str,
    user: UserId,
    name: &SecretName,
) -> Result<Option<Vec<u8>>, Fault> {
    if !store.front_lease_held(user, front)? {
        return Err(Fault::Refused(format!(
            "user {} holds no lease of {front}",
            user.get()
        )));
    }
    Ok(store
        .user_secret(vault, user, name)?
        .map(|s| s.expose().to_vec()))
}
