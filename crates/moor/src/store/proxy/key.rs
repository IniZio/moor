//! The key a Box's supervisor presents on the ledger socket. Only its hex SHA-256 is stored,
//! with the ledger protocol the supervisor was started to speak.

use moor_core::BoxId;
use moor_schema::tables::Proxy;
use sea_query::{Expr, ExprTrait as _, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::users::front_leases::hash;
use crate::store::{Store, StoreError};

impl Store {
    /// Records `key` as the one `box_id`'s next supervisor presents, speaking `ledger`; the key
    /// of any earlier supervisor stops working.
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
}

#[cfg(test)]
mod tests {
    use moor_core::domain::decision::{Decision, NewBox};
    use moor_core::BoxName;

    use crate::store::tests::open;
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
}
