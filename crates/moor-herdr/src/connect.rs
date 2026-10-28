//! Giving moor the secrets a Box needs.
//!
//! They are asked for when a start is refused for lacking them, and on demand to replace one. A
//! value is read with the terminal's echo off and goes only into the `secret-set` request.

use std::sync::Arc;

use moor_client::Client;
use moor_core::request::{AsReq, EnrollReq, Request, SecretSetReq, SecretValue};
use moor_core::user::SecretName;
use moor_core::view::user::{SecretNamesValue, StartNeedsValue};
use serde_json::{json, Value};

use crate::auth::login::{self, Ui};
use crate::auth::{normalize, Store, Tokens};
use crate::boxes::request;
use crate::error::Error;
use crate::picker::Prompt;

/// The controller calls that store a person's secrets, each as the person signed in.
pub trait Vault {
    /// What a start would need from the person, and what of it they lack.
    fn needs(&self) -> Result<StartNeedsValue, Error>;
    /// The names of the secrets the person has stored.
    fn names(&self) -> Result<Vec<SecretName>, Error>;
    /// Makes the person a moor user if they are none yet.
    fn enroll(&self) -> Result<(), Error>;
    /// Stores `value` as the person's secret `name`.
    fn store(&self, name: &SecretName, value: SecretValue) -> Result<(), Error>;
}

/// The reply to `req` when the controller took it, or its refusal.
fn ask(client: &Client, req: Request) -> Result<Value, Error> {
    let reply = client.request(req, |_| {})?;
    match reply.error {
        Some(e) => Err(Error::Refused(e.message)),
        None => Ok(reply.value.unwrap_or(Value::Null)),
    }
}

impl Vault for Client {
    fn needs(&self) -> Result<StartNeedsValue, Error> {
        let value = ask(self, request(json!({"cmd": "start-needs"}))?)?;
        Ok(serde_json::from_value(value)?)
    }

    fn names(&self) -> Result<Vec<SecretName>, Error> {
        let value = ask(self, Request::SecretNames(AsReq { identity: None }))?;
        Ok(serde_json::from_value::<SecretNamesValue>(value)?.names)
    }

    fn enroll(&self) -> Result<(), Error> {
        ask(self, Request::Enroll(EnrollReq { identity: None, email: None })).map(drop)
    }

    fn store(&self, name: &SecretName, value: SecretValue) -> Result<(), Error> {
        let set = SecretSetReq {
            identity: None,
            name: name.clone(),
            value,
        };
        ask(self, Request::SecretSet(set)).map(drop)
    }
}

/// Signs in to `server`, then makes the person a moor user there.
///
/// The gate adds who they are, so the request carries nothing of the person. `port` is where
/// the browser returns to; 0 takes any free one, and `now` is unix seconds.
///
/// # Errors
/// The sign-in fails, or it worked but moor did not answer the enrollment.
pub fn sign_in(
    server: &str,
    port: u16,
    store: &Store,
    ui: &mut dyn Ui,
    now: i64,
) -> Result<(), Error> {
    login::run(server, port, store, ui, now).map_err(Error::Usage)?;
    let server = normalize(server);
    ui.say(&format!("Signed in to {server}."));
    let tokens = Arc::new(Tokens::new(store.clone(), &server));
    Client::remote(server, tokens)
        .enroll()
        .map_err(|e| Error::Usage(format!("signed in, but moor did not answer: {e}")))
}

/// How the person is asked for the secret `name`: what it is and where to get it.
#[must_use]
pub fn guide(name: &str) -> (String, String) {
    match name {
        "claude" => (
            "Claude".to_owned(),
            "Paste the token that `claude setup-token` printed on your machine.".to_owned(),
        ),
        "github" => (
            "GitHub".to_owned(),
            "Paste the output of `gh auth token` from your machine.".to_owned(),
        ),
        other => (other.to_owned(), format!("Paste the value of {other}.")),
    }
}

/// Stores secrets, making the person a moor user before the first.
struct Session<'a> {
    /// Where they go.
    vault: &'a dyn Vault,
    /// Whether the person is known to be a moor user.
    enrolled: bool,
}

impl Session<'_> {
    /// Enrolls the person once.
    fn enroll(&mut self) -> Result<(), Error> {
        if !self.enrolled {
            self.vault.enroll()?;
            self.enrolled = true;
        }
        Ok(())
    }

    /// Stores `typed` as the secret `name`.
    fn put(&mut self, name: &SecretName, typed: String) -> Result<(), Error> {
        self.enroll()?;
        self.vault.store(name, SecretValue::new(typed.into_bytes()))
    }
}

/// Asks for one secret until something is typed; `None` when the input ends first.
fn ask_value(prompt: &mut dyn Prompt, name: &SecretName) -> Result<Option<String>, Error> {
    let (label, hint) = guide(name.as_str());
    prompt.say(&hint)?;
    loop {
        match prompt.ask_hidden(&format!("{label} token (hidden): "))? {
            Some(typed) if typed.is_empty() => prompt.say("Nothing typed.")?,
            other => return Ok(other),
        }
    }
}

/// Asks for every secret `needs` lacks and stores each; `false` when the input ended first.
pub fn provide(
    prompt: &mut dyn Prompt,
    vault: &dyn Vault,
    needs: &StartNeedsValue,
) -> Result<bool, Error> {
    let mut session = Session {
        vault,
        enrolled: false,
    };
    if needs.user.is_none() {
        session.enroll()?;
    }
    for name in &needs.missing {
        let Some(typed) = ask_value(prompt, name)? else {
            return Ok(false);
        };
        session.put(name, typed)?;
    }
    Ok(true)
}

/// `[connect]`: shows which secrets a start needs are stored and takes a new value for any the
/// person types one for, leaving the rest as they are.
pub fn run(prompt: &mut dyn Prompt, vault: &dyn Vault) -> Result<(), Error> {
    let needs = vault.needs()?;
    let stored = vault.names()?;
    let mut session = Session {
        vault,
        enrolled: needs.user.is_some(),
    };
    for name in &needs.needs {
        let (label, hint) = guide(name.as_str());
        let state = if stored.contains(name) {
            "set"
        } else {
            "not set"
        };
        prompt.say(&format!("{label}: {state}."))?;
        prompt.say(&hint)?;
        let question = format!("New {label} token (empty to keep): ");
        match prompt.ask_hidden(&question)? {
            None => return Ok(()),
            Some(typed) if typed.is_empty() => {}
            Some(typed) => {
                session.put(name, typed)?;
                prompt.say(&format!("{label} stored."))?;
            }
        }
    }
    if needs.needs.is_empty() {
        prompt.say("Nothing needs a token.")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
