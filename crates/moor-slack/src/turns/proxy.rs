//! A turn's lease of the front Proxy, which the controller runs and which holds the starter's
//! credential: leased before the agent starts, released when it ends, and asked about when a
//! turn fails, so a Proxy that is down is named in the thread.

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

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

impl FrontProxy {
    /// The Proxy at loopback `port`, with the CA's `pem`, when given, written to `proxy-ca.pem` in
    /// `state_dir` for the agent to trust; none when this front has no Proxy. A front that gets
    /// no `pem` here gets it with each lease.
    pub(crate) fn written(
        state_dir: &Path,
        port: Option<u16>,
        pem: Option<&str>,
    ) -> Result<Option<Self>, String> {
        let Some(port) = port else {
            return Ok(None);
        };
        let ca = state_dir.join("proxy-ca.pem");
        if let Some(pem) = pem {
            write_ca(&ca, pem)?;
        }
        Ok(Some(Self { port, ca }))
    }
}

/// Replaces the CA file `ca` with `pem`, in one step, since a turn may be reading it.
fn write_ca(ca: &Path, pem: &str) -> Result<(), String> {
    let partial = ca.with_extension("pem.partial");
    let fail = |e: std::io::Error| format!("{}: {e}", ca.display());
    let _ = std::fs::remove_file(&partial);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&partial)
        .map_err(fail)?;
    file.write_all(pem.as_bytes()).map_err(fail)?;
    std::fs::rename(&partial, ca).map_err(fail)
}

/// Leases the Proxy as `client`'s identity, the turn's `starter`, keeping in `ca` any CA the
/// lease comes with; the closing of a turn that gets no lease otherwise.
pub(super) fn lease(client: &Client, starter: &str, ca: &Path) -> Result<Secret, Closing> {
    let failed = |line: String| Closing::of(End::Failed, line);
    let lease = call(client, Request::ProxyLease(AsReq { identity: None }))
        .and_then(|v| serde_json::from_value(v).map_err(|e| format!("the lease reply: {e}")))
        .map_err(|e| failed(lines::unavailable(&format!("no lease: {e}"))))?;
    match lease {
        ProxyLeaseValue::Granted { token, ca_pem, .. } => {
            if let Some(pem) = ca_pem {
                write_ca(ca, &pem).map_err(|e| failed(lines::unavailable(&e)))?;
            }
            Ok(Secret::new(token.secret().expose().to_vec()))
        }
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

#[cfg(test)]
mod ca_tests {
    #![allow(clippy::unwrap_used, reason = "test setup")]

    use super::{write_ca, FrontProxy};

    #[test]
    fn a_port_with_no_pem_is_a_proxy_whose_ca_comes_later_and_replaces_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let proxy = FrontProxy::written(dir.path(), Some(4480), None)
            .unwrap()
            .unwrap();
        assert_eq!(proxy.port, 4480);
        assert!(!proxy.ca.exists());
        write_ca(&proxy.ca, "first").unwrap();
        write_ca(&proxy.ca, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&proxy.ca).unwrap(), "second");
        assert!(FrontProxy::written(dir.path(), None, Some("pem"))
            .unwrap()
            .is_none());
    }
}
