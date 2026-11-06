//! The token a Box presents to the shared egress Proxy. Only its hex SHA-256 is stored.

use moor_core::BoxId;
use moor_schema::tables::Proxy;
use sea_query::{Expr, ExprTrait as _, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::users::front_leases::hash;
use crate::store::{Store, StoreError};

impl Store {
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

    /// Records that `token`, minted under the key `key_id` names, reached `box_id`; false when
    /// the row holds another token, minted after it.
    pub(crate) fn note_egress_token_delivered(
        &self,
        box_id: BoxId,
        token: &[u8],
        key_id: i64,
    ) -> Result<bool, StoreError> {
        let changed = self.writer().run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::TokenDelivered, key_id)
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

    /// Whether the egress token the row of `box_id` holds reached the Box and was minted under
    /// the key `key_id` names; a token under another key no longer verifies.
    pub(crate) fn egress_token_delivered(
        &self,
        box_id: BoxId,
        key_id: i64,
    ) -> Result<bool, StoreError> {
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
        Ok(delivered.flatten() == Some(key_id))
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

    /// Every egress token hash the store keeps, with its Box.
    pub(crate) fn egress_tokens(&self) -> Result<Vec<(String, BoxId)>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .columns([Proxy::TokenHash, Proxy::Box])
                .from(Proxy::Table)
                .and_where(Expr::col(Proxy::TokenHash).is_not_null()),
            |r| Ok::<_, StoreError>((r.get::<_, String>(0)?, BoxId::new(r.get(1)?))),
        )?;
        rows.collect()
    }
}

#[cfg(test)]
impl Store {
    /// Writes `state` into the row of `box_id` unchecked, as a damaged row would hold it.
    pub(crate) fn damage_box_state(&self, box_id: BoxId, state: &str) {
        use moor_schema::tables::Boxes;
        self.writer()
            .run(
                Query::update()
                    .table(Boxes::Table)
                    .value(Boxes::State, state)
                    .and_where(Expr::col(Boxes::Id).eq(box_id.get())),
            )
            .unwrap();
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
    fn a_new_egress_token_replaces_the_last_until_the_row_goes() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open(dir.path());
        let nt = NewBox {
            name: BoxName::parse_allow_bare("a").unwrap(),
            repo: None,
        };
        let a = store.apply(Decision::insert(nt)).unwrap().box_id;
        assert!(!store.set_egress_token(a, b"t1").unwrap(), "no row yet");
        drop(store.reserve(a, &FakeRng::default()));
        assert!(store.set_egress_token(a, b"t1").unwrap());
        assert!(store.note_egress_token_delivered(a, b"t1", 1).unwrap());
        assert!(
            store.set_egress_token(a, b"t2").unwrap(),
            "a lost token is replaced"
        );
        assert_eq!(store.egress_token_box(&hash(b"t1")).unwrap(), None);
        assert!(
            !store.note_egress_token_delivered(a, b"t1", 1).unwrap(),
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
