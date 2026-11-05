//! What `ensure` records before a supervisor starts, and the endpoint it hands back once the
//! supervisor is ready.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::Path;

use moor_core::seam::proxy_host::{ProxyEndpoint, ProxyHostError, ProxySpec};

use super::{fail, store_failed};
use crate::store::{ProxyRow, Store};

/// Records `spec` as what the Box's Proxy starts from.
pub(super) fn record(store: &Store, spec: &ProxySpec, log: &Path) -> Result<(), ProxyHostError> {
    store
        .set_proxy_spec(spec.box_id, spec)
        .map_err(|e| store_failed(&e, log))
}

/// The endpoint a ready row describes, with the CA its supervisor serves with.
pub(super) fn ready_endpoint(
    store: &Store,
    row: &ProxyRow,
    log: &Path,
) -> Result<ProxyEndpoint, ProxyHostError> {
    let ca = store
        .proxy_ca(row.box_id)
        .map_err(|e| store_failed(&e, log))?
        .ok_or_else(|| fail("the ready proxy has no CA", log))?;
    Ok(ProxyEndpoint {
        box_port: row.box_port,
        egress_port: row.egress_port,
        forward_port: row.forward_port,
        ca_pem: ca.cert_pem,
    })
}

/// Creates `dir` and its parents, and makes `dir` owner-only.
pub(super) fn owner_only_dir(dir: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}
