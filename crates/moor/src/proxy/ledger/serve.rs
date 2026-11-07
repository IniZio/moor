//! The controller's end of the ledger: one connection, its hello, then its calls.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;

use moor_core::user::{SecretName, UserId};
use serde::Serialize;
use zeroize::Zeroizing;

use super::{CaPair, Call, Fault, FrontLease, Hello, LEDGER};
use crate::env::Env;
use crate::store::Store;
use crate::vault::Vault;

mod boxes;
mod start;
pub(crate) use start::ca as box_ca;

/// The longest hello read; anyone who can reach the TLS listener can send one.
const HELLO_MAX: u64 = 64 * 1024;

/// The keys the controller started its Proxy processes with, and who may say hello.
#[derive(Default)]
pub(crate) struct Keys<'a> {
    /// The front Proxy's key; none when no front Proxy runs.
    pub(crate) fronts: Option<&'a str>,
    /// The shared egress Proxy's key; none while the shared Proxy is off.
    pub(crate) egress: Option<&'a str>,
    /// Which fronts run elsewhere, and the rules and state dir they share; none answers no call
    /// about a remote front.
    pub(crate) env: Option<&'a Env>,
}

/// Whom a connection's hello proved it speaks for.
enum Bound {
    /// The front Proxy.
    Fronts,
    /// The shared egress Proxy, for one Box or for token lookups.
    Egress(Option<moor_core::BoxId>),
}

/// Serves one connection until EOF or a failed write.
pub(crate) fn serve((store, vault): (&Store, &Vault), keys: &Keys<'_>, stream: impl Read + Write) {
    let mut read = BufReader::new(stream);
    let Some(line) = next(&mut (&mut read).take(HELLO_MAX)) else {
        return;
    };
    let bound = match hello(keys, &line) {
        Ok(bound) => bound,
        Err(fault) => {
            send(read.get_mut(), &encode(&Err::<(), _>(fault)));
            return;
        }
    };
    if !send(read.get_mut(), &encode(&Ok::<(), Fault>(()))) {
        return;
    }
    while let Some(line) = next(&mut read) {
        if !send(
            read.get_mut(),
            &answer((store, vault), keys.env, &bound, &line),
        ) {
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
        Hello::Egress { v, key, box_id } => {
            speaks(v)?;
            let want = keys
                .egress
                .ok_or_else(|| Fault::Refused("the shared egress Proxy is off".to_owned()))?;
            if !same(want, &key) {
                return Err(Fault::Refused("not the egress Proxy's key".to_owned()));
            }
            Ok(Bound::Egress(box_id))
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

/// Whether two keys match, compared so timing says nothing of the key.
fn same(want: &str, got: &str) -> bool {
    crate::env::same_key(want, got)
}

/// The reply to `line`, a call by `bound`.
fn answer(
    (store, vault): (&Store, &Vault),
    env: Option<&Env>,
    bound: &Bound,
    line: &str,
) -> Zeroizing<String> {
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
        (Bound::Egress(None), Call::FrontLeaseOf { token_hash }) => {
            encode(&front_lease_of(store, env, &token_hash))
        }
        (Bound::Egress(None), Call::FrontCa) => encode(&front_ca(env)),
        (Bound::Egress(None), Call::FrontSecret { front, user, name }) => {
            if env.is_some_and(|env| env.front_is_remote(&front)) {
                encode(&front_secret((store, vault), &front, user, &name))
            } else {
                encode(&Err::<(), _>(not_remote(&front)))
            }
        }
        (Bound::Egress(None), Call::TokenBox { token_hash }) => {
            encode(&boxes::token_box(store, &token_hash).map_err(Fault::from))
        }
        (Bound::Egress(None), Call::AwakeTokens) => {
            encode(&boxes::awake_tokens(store).map_err(Fault::from))
        }
        (Bound::Egress(Some(box_id)), call) => boxes::for_egress((store, vault), *box_id, call),
        (Bound::Fronts, _) => encode(&Err::<(), _>(Fault::Refused(
            "the front Proxy makes no Box call".to_owned(),
        ))),
        (Bound::Egress(None), _) => encode(&Err::<(), _>(Fault::Refused(
            "a token lookup connection makes no Box call".to_owned(),
        ))),
    }
}

/// The refusal for a call about `front`, whose Proxy is not the shared one.
fn not_remote(front: &str) -> Fault {
    Fault::Refused(format!("{front} is not a remote front"))
}

/// The live lease whose token hashes to `token_hash`, when its front runs elsewhere; a lease of
/// a front beside the controller is spent only through that front's own Proxy.
fn front_lease_of(
    store: &Store,
    env: Option<&Env>,
    token_hash: &str,
) -> Result<Option<FrontLease>, Fault> {
    let Some(env) = env else { return Ok(None) };
    Ok(store
        .front_lease_of(token_hash)?
        .filter(|(front, _)| env.front_is_remote(front))
        .map(|(front, user)| FrontLease {
            rules: env.front_proxy_rules(&front).to_vec(),
            front,
            user,
        }))
}

/// The CA every remote front's Proxy signs with.
fn front_ca(env: Option<&Env>) -> Result<CaPair, Fault> {
    let state: PathBuf = env
        .and_then(Env::state_dir)
        .ok_or_else(|| Fault::Store("a front Proxy needs a state dir".to_owned()))?;
    crate::proxy::fronts::ca_pair(&state).map_err(|e| Fault::Store(e.to_string()))
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
