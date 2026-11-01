//! `moor_` API tokens: the controller says whose a token is, and `/mcp` remembers the answer
//! for [`TTL`] so a polling bot does not reach the controller on every request.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use moor_client::{Client, Redial};
use moor_core::request::{ApiTokenResolveReq, Request, SecretValue};
use moor_core::response::ExitClass;
use moor_core::view::user::ApiTokenResolveValue;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use crate::Person;

/// How every API token's text starts.
pub(crate) const PREFIX: &str = "moor_";

/// How long a resolved token is trusted without asking again; a revoked token keeps working
/// this long at most.
const TTL: Duration = Duration::from_secs(30);

/// The most resolved tokens remembered at once; the oldest goes first.
const MAX: usize = 1024;

/// How long the gate keeps dialing a controller that does not answer.
const DIAL_BUDGET: Duration = Duration::from_secs(2);

/// Why a request to the controller gave no value.
#[derive(Debug)]
pub(crate) enum Failure {
    /// The controller answered with a refusal.
    Refused(String),
    /// The controller could not be reached or answered nonsense.
    Unavailable(String),
}

/// A token's owner and scopes, as the controller last said.
struct Resolved {
    /// The owner.
    person: Person,
    /// What the token may do.
    scopes: Vec<String>,
    /// When the controller said so.
    at: Instant,
}

/// Resolved tokens, keyed by the SHA-256 of their text so no token text is kept.
#[derive(Default)]
pub(crate) struct Cache(Mutex<HashMap<[u8; 32], Resolved>>);

impl Cache {
    /// Whose `token` is and what it may do, asking the controller on `socket` unless a
    /// fresh answer is remembered.
    pub(crate) fn resolve(
        &self,
        socket: &Path,
        token: &str,
        now: Instant,
    ) -> Result<(Person, Vec<String>), Failure> {
        let key: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        if let Some(hit) = self.lookup(&key, now) {
            return Ok(hit);
        }
        let request = Request::ApiTokenResolve(ApiTokenResolveReq {
            token: SecretValue::new(token.as_bytes().to_vec()),
        });
        let value: ApiTokenResolveValue = ask(socket, request)?;
        let email = value.email.ok_or_else(|| {
            Failure::Refused("the token's person has no verified email".to_owned())
        })?;
        let person = Person {
            identity: value.identity,
            email,
        };
        self.keep(key, person.clone(), value.scopes.clone(), now);
        Ok((person, value.scopes))
    }

    /// The remembered answer for `key`, while it is fresh.
    fn lookup(&self, key: &[u8; 32], now: Instant) -> Option<(Person, Vec<String>)> {
        let all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        all.get(key)
            .filter(|r| now.saturating_duration_since(r.at) < TTL)
            .map(|r| (r.person.clone(), r.scopes.clone()))
    }

    /// Remembers an answer, dropping stale ones and then the oldest past [`MAX`].
    fn keep(&self, key: [u8; 32], person: Person, scopes: Vec<String>, now: Instant) {
        let mut all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        all.retain(|_, r| now.saturating_duration_since(r.at) < TTL);
        while all.len() >= MAX {
            let oldest = all.iter().min_by_key(|(_, r)| r.at).map(|(k, _)| *k);
            match oldest {
                Some(k) => all.remove(&k),
                None => break,
            };
        }
        all.insert(
            key,
            Resolved {
                person,
                scopes,
                at: now,
            },
        );
    }
}

/// Sends `request` to the controller on `socket`: its value, or why not.
pub(crate) fn ask<T: DeserializeOwned>(socket: &Path, request: Request) -> Result<T, Failure> {
    let reply = Client::new(socket)
        .with_redial(Redial::within(DIAL_BUDGET))
        .request(request, |_| {})
        .map_err(|e| Failure::Unavailable(format!("the controller is unreachable: {e}")))?;
    if reply.class != ExitClass::Ok {
        return Err(Failure::Refused(reply.error.map_or_else(
            || "the controller refused".to_owned(),
            |e| e.message,
        )));
    }
    serde_json::from_value(reply.value.unwrap_or_default())
        .map_err(|e| Failure::Unavailable(format!("the controller's answer is unreadable: {e}")))
}
