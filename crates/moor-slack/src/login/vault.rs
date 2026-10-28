//! What `/moor login` and `/moor logout` ask of the controller, each as the Slack user it
//! concerns: enroll and store the secret, or delete it.

use moor_client::ClientError;
use moor_core::request::{EnrollReq, Request, SecretDeleteReq, SecretSetReq, SecretValue};
use moor_core::response::{ExitClass, Response};
use moor_core::secret::Secret;
use serde_json::Value;

use crate::account::Account;

/// Sends one request to the controller as the Slack user it names.
pub(crate) type Call<'a> = dyn FnMut(&str, Request) -> Result<Response, ClientError> + 'a;

/// Creates `user`'s moor user when it has none, then stores `token` as its secret for
/// `account`. The order matters: the controller refuses `secret-set` from a user not yet
/// enrolled.
pub(crate) fn store(
    call: &mut Call<'_>,
    user: &str,
    account: Account,
    token: &Secret,
) -> Result<(), String> {
    let name = account.secret()?;
    send(
        call,
        user,
        Request::Enroll(EnrollReq {
            identity: None,
            email: None,
        }),
    )?;
    let set = SecretSetReq {
        identity: None,
        name,
        value: SecretValue::new(token.expose().to_vec()),
    };
    send(call, user, Request::SecretSet(set)).map(drop)
}

/// Deletes `user`'s secret for `account`; whether there was one.
pub(crate) fn delete(call: &mut Call<'_>, user: &str, account: Account) -> Result<bool, String> {
    let delete = SecretDeleteReq {
        identity: None,
        name: account.secret()?,
    };
    let reply = send(call, user, Request::SecretDelete(delete))?;
    let changed = reply.value.as_ref().and_then(|v| v.get("changed"));
    Ok(changed.and_then(Value::as_bool).unwrap_or(false))
}

/// The reply to `request` when the controller took it, or its error text.
pub(crate) fn send(call: &mut Call<'_>, user: &str, request: Request) -> Result<Response, String> {
    let reply = call(user, request).map_err(|e| e.to_string())?;
    if reply.class == ExitClass::Ok {
        return Ok(reply);
    }
    Err(reply
        .error
        .map_or_else(|| format!("{} failed", reply.cmd), |error| error.message))
}
