//! A turn's lease of the front Proxy, which the controller runs and which holds the starter's
//! credential: leased before the agent starts, released when it ends, and asked about when a
//! turn fails, so a Proxy that is down is named in the thread.

use std::path::PathBuf;

use moor_client::Client;
use moor_core::request::{AsReq, ProxyReleaseReq, Request, SecretValue};
use moor_core::response::ExitClass;
use moor_core::secret::Secret;
use moor_core::view::user::{ProxyLeaseValue, ProxyStatusValue};

use super::{lines, Closing};
use crate::coordinator::turn::Ended;
use crate::coordinator::Answer;
use crate::db::End;

/// Where this front reaches its Proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontProxy {
    /// The loopback port.
    pub(crate) port: u16,
    /// The CA certificate.
    pub(crate) ca: PathBuf,
}

/// Leases the Proxy as `client`'s identity, the turn's `starter`; the closing of a turn that
/// gets no lease otherwise.
pub(super) fn lease(client: &Client, starter: &str) -> Result<Secret, Closing> {
    let failed = |line: String| Closing::of(End::Failed, line);
    let lease = call(client, Request::ProxyLease(AsReq { identity: None }))
        .and_then(|v| serde_json::from_value(v).map_err(|e| format!("the lease reply: {e}")))
        .map_err(|e| failed(lines::unavailable(&format!("no lease: {e}"))))?;
    match lease {
        ProxyLeaseValue::Granted { token, .. } => Ok(Secret::new(token.secret().expose().to_vec())),
        ProxyLeaseValue::NoSecret { .. } => {
            Err(Closing::of(End::NoToken, lines::no_token(starter)))
        }
        ProxyLeaseValue::ProxyDown => Err(failed(lines::PROXY_DOWN.to_owned())),
    }
}

/// Whether a turn that ended as `ended` failed while the controller says the Proxy is down.
pub(super) fn failed_while_down(client: &Client, ended: &Ended) -> bool {
    let failed = matches!(
        ended,
        Ended::Answered(Answer::Failed(_) | Answer::Unavailable(_))
    );
    failed && down(client)
}

/// Ends the lease `token`; a failure only leaves it to expire.
pub(super) fn release(client: &Client, token: &Secret) {
    let release = ProxyReleaseReq {
        identity: None,
        token: SecretValue::new(token.expose().to_vec()),
    };
    let _ = call(client, Request::ProxyRelease(release));
}

/// Whether the controller says the Proxy is not running; an unclear answer is not a no.
fn down(client: &Client) -> bool {
    call(client, Request::ProxyStatus(AsReq { identity: None }))
        .ok()
        .and_then(|value| serde_json::from_value::<ProxyStatusValue>(value).ok())
        .is_some_and(|status| !status.running)
}

/// The value of the reply to `request`, or its error text.
fn call(client: &Client, request: Request) -> Result<serde_json::Value, String> {
    let reply = client.request(request, |_| {}).map_err(|e| e.to_string())?;
    if reply.class == ExitClass::Ok {
        return Ok(reply.value.unwrap_or(serde_json::Value::Null));
    }
    Err(reply
        .error
        .map_or_else(|| format!("{} failed", reply.cmd), |e| e.message))
}
