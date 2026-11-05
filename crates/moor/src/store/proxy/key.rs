//! The key a Box's supervisor presents on the ledger socket, and the token the Box presents to
//! the shared egress Proxy. Only their hex SHA-256 is stored, with the ledger protocol the
//! supervisor was started to speak.

use moor_core::BoxId;
use moor_schema::tables::Proxy;
use sea_query::{Expr, ExprTrait as _, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::users::front_leases::hash;
use crate::store::{Store, StoreError};

impl Store {
    /// Records `key` as the one `box_id`'s next supervisor presents, speaking `ledger`; the key
    /// of any earlier supervisor stops working.
    #[cfg(test)]
    pub(crate) fn set_proxy_key(
        &self,
        box_id: BoxId,
        key: &[u8],
        ledger: u32,
    ) -> Result<(), StoreError> {
        let changed = self.writer().run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::KeyHash, hash(key))
                .value(Proxy::Ledger, ledger)
                .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(box_id));
        }
        Ok(())
    }

    /// Whether `key` is the one `box_id`'s supervisor was started with.
    pub(crate) fn proxy_key_is(&self, box_id: BoxId, key: &[u8]) -> Result<bool, StoreError> {
        let found: Option<i64> = self
            .reader()?
            .row(
                Query::select()
                    .column(Proxy::Box)
                    .from(Proxy::Table)
                    .and_where(Expr::col(Proxy::Box).eq(box_id.get()))
                    .and_where(Expr::col(Proxy::KeyHash).eq(hash(key))),
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Records `token` as the one `box_id` presents to the shared egress Proxy, replacing any
    /// earlier one, which stops working; false when the Box has no proxy row. Removing the row
    /// revokes the token.
    pub(crate) fn set_egress_token(&self, box_id: BoxId, token: &[u8]) -> Result<bool, StoreError> {
        let changed = self.writer().run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::TokenHash, hash(token))
                .value(Proxy::TokenDelivered, 0)
                .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
        )?;
        Ok(changed > 0)
    }

    /// Records that `token` reached `box_id`; false when the row holds another token, minted
    /// after it.
    pub(crate) fn note_egress_token_delivered(
        &self,
        box_id: BoxId,
        token: &[u8],
    ) -> Result<bool, StoreError> {
        let changed = self.writer().run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::TokenDelivered, 1)
                .and_where(Expr::col(Proxy::Box).eq(box_id.get()))
                .and_where(Expr::col(Proxy::TokenHash).eq(hash(token))),
        )?;
        Ok(changed > 0)
    }

    /// Marks the egress token of `box_id` undelivered, so the next exec delivers the forwarder
    /// again.
    pub(crate) fn forget_egress_delivery(&self, box_id: BoxId) -> Result<(), StoreError> {
        super::forget_egress_delivery(&*self.writer(), box_id)
    }

    /// Whether the egress token the row of `box_id` holds reached the Box.
    pub(crate) fn egress_token_delivered(&self, box_id: BoxId) -> Result<bool, StoreError> {
        let delivered: Option<Option<i64>> = self
            .reader()?
            .row(
                Query::select()
                    .column(Proxy::TokenDelivered)
                    .from(Proxy::Table)
                    .and_where(Expr::col(Proxy::Box).eq(box_id.get())),
                |r| r.get(0),
            )
            .optional()?;
        Ok(delivered.flatten().is_some_and(|d| d != 0))
    }

    /// The Box whose egress token hashes to `token_hash`, if a row holds it.
    pub(crate) fn egress_token_box(&self, token_hash: &str) -> Result<Option<BoxId>, StoreError> {
        let found: Option<i64> = self
            .reader()?
            .row(
                Query::select()
                    .column(Proxy::Box)
                    .from(Proxy::Table)
                    .and_where(Expr::col(Proxy::TokenHash).eq(token_hash)),
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.map(BoxId::new))
    }
}

#[cfg(test)]
mod tests {
    use moor_core::domain::decision::{Decision, NewBox};
    use moor_core::BoxName;

    use crate::store::tests::open;
    use crate::store::users::front_leases::hash;
    use crate::testing::rng::FakeRng;

    #[test]
    fn only_the_latest_key_of_a_box_matches() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open(dir.path());
        let new = |name: &str| {
            let nt = NewBox {
                name: BoxName::parse_allow_bare(name).unwrap(),
                repo: None,
            };
            store.apply(Decision::insert(nt)).unwrap().box_id
        };
        let (a, b) = (new("a"), new("b"));
        for id in [a, b] {
            drop(store.reserve(id, &FakeRng::default(), std::time::Duration::from_secs(30)));
        }
        assert!(!store.proxy_key_is(a, b"k1").unwrap(), "no key yet");
        store.set_proxy_key(a, b"k1", 1).unwrap();
        assert!(store.proxy_key_is(a, b"k1").unwrap());
        assert!(!store.proxy_key_is(b, b"k1").unwrap(), "another Box");
        store.set_proxy_key(a, b"k2", 1).unwrap();
        assert!(!store.proxy_key_is(a, b"k1").unwrap(), "replaced");
        assert!(store.proxy_key_is(a, b"k2").unwrap());
        assert_eq!(store.proxy(a).unwrap().unwrap().ledger, Some(1));
        assert_eq!(store.proxy(b).unwrap().unwrap().ledger, None);
    }

    #[test]
    fn a_new_egress_token_replaces_the_last_until_the_row_goes() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open(dir.path());
        let nt = NewBox {
            name: BoxName::parse_allow_bare("a").unwrap(),
            repo: None,
        };
        let a = store.apply(Decision::insert(nt)).unwrap().box_id;
        assert!(!store.set_egress_token(a, b"t1").unwrap(), "no row yet");
        drop(store.reserve(a, &FakeRng::default(), std::time::Duration::from_secs(30)));
        assert!(store.set_egress_token(a, b"t1").unwrap());
        assert!(store.note_egress_token_delivered(a, b"t1").unwrap());
        assert!(
            store.set_egress_token(a, b"t2").unwrap(),
            "a lost token is replaced"
        );
        assert_eq!(store.egress_token_box(&hash(b"t1")).unwrap(), None);
        assert!(
            !store.note_egress_token_delivered(a, b"t1").unwrap(),
            "a stale delivery"
        );
        assert_eq!(store.egress_token_box(&hash(b"t2")).unwrap(), Some(a));
        store.remove_proxy(a).unwrap();
        assert_eq!(
            store.egress_token_box(&hash(b"t2")).unwrap(),
            None,
            "revoked"
        );
    }
}
