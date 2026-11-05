//! What a Proxy is started from, kept in its row: the spec, and the interception CA the Box
//! trusts for its whole life, with the CA key sealed by the vault.

use moor_core::seam::proxy_host::ProxySpec;
use moor_core::BoxId;
use moor_schema::tables::Proxy;
use sea_query::{Expr, ExprTrait as _, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

/// A Box's interception CA as the row keeps it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoredCa {
    /// The PEM certificate the Box trusts.
    pub(crate) cert_pem: String,
    /// The PEM private key, sealed by the vault.
    pub(crate) sealed_key: Vec<u8>,
}

impl std::fmt::Debug for StoredCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoredCa(..)")
    }
}

impl Store {
    /// Records `spec` as what `box_id`'s Proxy starts from.
    pub(crate) fn set_proxy_spec(&self, box_id: BoxId, spec: &ProxySpec) -> Result<(), StoreError> {
        let json = serde_json::to_string(spec).map_err(|e| StoreError::Invalid(e.to_string()))?;
        let changed = self.writer().run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::Spec, json)
                .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(box_id));
        }
        Ok(())
    }

    /// The spec `box_id`'s Proxy starts from; none before the first is recorded.
    pub(crate) fn proxy_spec(&self, box_id: BoxId) -> Result<Option<ProxySpec>, StoreError> {
        let json: Option<Option<String>> = self
            .reader()?
            .row(
                Query::select()
                    .column(Proxy::Spec)
                    .from(Proxy::Table)
                    .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
                |r| r.get(0),
            )
            .optional()?;
        json.flatten()
            .map(|j| serde_json::from_str(&j))
            .transpose()
            .map_err(|e| StoreError::Corrupt(format!("proxy spec of Box {}: {e}", box_id.get())))
    }

    /// `box_id`'s CA; none before the first is kept.
    pub(crate) fn proxy_ca(&self, box_id: BoxId) -> Result<Option<StoredCa>, StoreError> {
        let found: Option<(Option<String>, Option<Vec<u8>>)> = self
            .reader()?
            .row(
                Query::select()
                    .columns([Proxy::CaCert, Proxy::CaKey])
                    .from(Proxy::Table)
                    .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match found {
            Some((Some(cert_pem), Some(sealed_key))) => Some(StoredCa {
                cert_pem,
                sealed_key,
            }),
            _ => None,
        })
    }

    /// Keeps `ca` as `box_id`'s CA unless it already has one; returns the CA it has after.
    ///
    /// The Box trusts the first CA for its whole life, so a later one never replaces it.
    pub(crate) fn keep_proxy_ca(
        &self,
        box_id: BoxId,
        ca: &StoredCa,
    ) -> Result<StoredCa, StoreError> {
        self.writer().run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::CaCert, ca.cert_pem.as_str())
                .value(Proxy::CaKey, ca.sealed_key.clone())
                .and_where(Expr::col(Proxy::Box).eq(box_id.get()))
                .and_where(Expr::col(Proxy::CaCert).is_null()),
        )?;
        self.proxy_ca(box_id)?.ok_or(StoreError::NotFound(box_id))
    }
}

#[cfg(test)]
mod tests {
    use moor_core::domain::decision::{Decision, NewBox};
    use moor_core::seam::proxy_host::ProxySpec;
    use moor_core::user::UserId;
    use moor_core::{BoxId, BoxName};

    use super::StoredCa;
    use crate::store::tests::open;
    use crate::store::{Store, StoreError};
    use crate::testing::rng::FakeRng;

    fn reserved(store: &Store, name: &str) -> BoxId {
        let nt = NewBox {
            name: BoxName::parse_allow_bare(name).unwrap(),
            repo: None,
        };
        let id = store.apply(Decision::insert(nt)).unwrap().box_id;
        drop(store.reserve(id, &FakeRng::default()));
        id
    }

    fn spec(id: BoxId) -> ProxySpec {
        ProxySpec {
            box_id: id,
            box_handle: None,
            other_repos: Vec::new(),
            box_repo: None,
            rules: Vec::new(),
            user: UserId::new(1),
        }
    }

    fn ca(cert: &str) -> StoredCa {
        StoredCa {
            cert_pem: cert.to_owned(),
            sealed_key: cert.as_bytes().to_vec(),
        }
    }

    #[test]
    fn the_spec_a_proxy_starts_from_is_the_latest_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open(dir.path());
        let id = reserved(&store, "a");
        assert_eq!(store.proxy_spec(id).unwrap(), None);
        store.set_proxy_spec(id, &spec(id)).unwrap();
        let newer = ProxySpec {
            other_repos: vec![moor_core::seam::forge::ForgeRepo::new("o", "n")],
            ..spec(id)
        };
        store.set_proxy_spec(id, &newer).unwrap();
        assert_eq!(store.proxy_spec(id).unwrap(), Some(newer));
    }

    #[test]
    fn a_spec_for_a_box_without_a_proxy_row_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open(dir.path());
        let id = BoxId::new(99);
        assert!(matches!(
            store.set_proxy_spec(id, &spec(id)),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn the_first_ca_kept_for_a_box_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open(dir.path());
        let id = reserved(&store, "a");
        assert_eq!(store.proxy_ca(id).unwrap(), None);
        assert_eq!(store.keep_proxy_ca(id, &ca("first")).unwrap(), ca("first"));
        assert_eq!(store.keep_proxy_ca(id, &ca("second")).unwrap(), ca("first"));
        assert_eq!(store.proxy_ca(id).unwrap(), Some(ca("first")));
    }

    #[test]
    fn a_reserve_again_keeps_the_spec_and_ca() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open(dir.path());
        let id = reserved(&store, "a");
        store.set_proxy_spec(id, &spec(id)).unwrap();
        store.keep_proxy_ca(id, &ca("first")).unwrap();
        drop(store.reserve(id, &FakeRng::default()));
        assert_eq!(store.proxy_spec(id).unwrap(), Some(spec(id)));
        assert_eq!(store.proxy_ca(id).unwrap(), Some(ca("first")));
    }
}
