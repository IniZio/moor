//! What a Box's engine starts from: its spec, and its interception CA, which the controller
//! makes on the first ask and keeps sealed in the Proxy row for the Box's whole life.

use moor_core::BoxId;
use moor_core::Secret;

use super::super::{CaPair, Fault};
use crate::proxy::nono::ca;
use crate::store::{Store, StoredCa};
use crate::vault::Vault;

/// `box_id`'s CA, opened; made and kept first when the row has none.
pub(crate) fn ca((store, vault): (&Store, &Vault), box_id: BoxId) -> Result<CaPair, Fault> {
    let stored = if let Some(stored) = store.proxy_ca(box_id)? {
        stored
    } else {
        let (key_pem, cert_pem) = ca::generate(box_id).map_err(|e| Fault::Store(e.to_string()))?;
        let key = Secret::new(key_pem.as_bytes().to_vec());
        let sealed_key = vault
            .seal_box_ca(box_id, &key)
            .map_err(|e| Fault::Store(e.to_string()))?;
        store.keep_proxy_ca(
            box_id,
            &StoredCa {
                cert_pem,
                sealed_key,
            },
        )?
    };
    let key = vault
        .open_box_ca(box_id, &stored.sealed_key)
        .map_err(|e| Fault::Store(e.to_string()))?;
    Ok(CaPair {
        key_pem: key.expose().to_vec(),
        cert_pem: stored.cert_pem,
    })
}
