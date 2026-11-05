//! Moves what a Proxy was started from before its row kept it, a spec file and the CA files under
//! the state dir, into the row. A Box trusts its CA for life, so a row that lost it would break the
//! Box at its next Proxy restart.

use std::fs;
use std::path::Path;

use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxId, Secret};

use crate::proxy::nono::ca::{ca_dir, cert_in, key_in};
use crate::store::{Store, StoredCa};
use crate::vault::Vault;

/// The spec file a Proxy was started from before its row kept the spec.
const SPEC_FILE: &str = "spec.json";

/// Imports the files of every row that lacks its spec or CA; returns one line per failure.
pub(crate) fn import(store: &Store, vault: &Vault, state_dir: &Path) -> Vec<String> {
    let rows = match store.proxies() {
        Ok(rows) => rows,
        Err(e) => return vec![format!("cannot read the proxy state: {e}")],
    };
    rows.iter()
        .filter_map(|row| {
            import_one(store, vault, &ca_dir(state_dir, row.box_id), row.box_id)
                .err()
                .map(|e| format!("Box {}: {e}", row.box_id.get()))
        })
        .collect()
}

/// Imports `box_id`'s spec and CA from `dir`, each only when the row lacks it and the files exist.
fn import_one(store: &Store, vault: &Vault, dir: &Path, box_id: BoxId) -> Result<(), String> {
    if store
        .proxy_spec(box_id)
        .map_err(|e| e.to_string())?
        .is_none()
    {
        if let Ok(bytes) = fs::read(dir.join(SPEC_FILE)) {
            let spec: ProxySpec = serde_json::from_slice(&bytes)
                .map_err(|e| format!("the proxy spec file is not valid: {e}"))?;
            store
                .set_proxy_spec(box_id, &spec)
                .map_err(|e| e.to_string())?;
        }
    }
    if store.proxy_ca(box_id).map_err(|e| e.to_string())?.is_some() {
        return Ok(());
    }
    let (Ok(key), Ok(cert_pem)) = (fs::read(key_in(dir)), fs::read_to_string(cert_in(dir))) else {
        return Ok(());
    };
    let sealed_key = vault
        .seal_box_ca(box_id, &Secret::new(key))
        .map_err(|e| e.to_string())?;
    store
        .keep_proxy_ca(
            box_id,
            &StoredCa {
                cert_pem,
                sealed_key,
            },
        )
        .map(drop)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
