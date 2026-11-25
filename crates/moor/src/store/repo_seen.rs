//! The Proxy events of a repo's Boxes, for the lists beside its rules.

use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_schema::tables::{Boxes, Events};
use sea_query::{Expr, ExprTrait as _, Order, Query};

use crate::store::db::DbExt as _;
use crate::store::{Store, StoreError};

/// The most events one read returns.
const MOST: u64 = 10_000;

impl Store {
    /// The `credential_withheld` and `egress_seen` events of the last `days` days of every Box
    /// whose first repo is `repo`, oldest first, as (Box, kind, payload).
    pub(crate) fn repo_proxy_events(
        &self,
        repo: &ForgeRepo,
        days: u32,
    ) -> Result<Vec<(i64, String, serde_json::Value)>, StoreError> {
        let since = self.now().as_millis() - i64::from(days) * 86_400_000;
        let e = |c| (Events::Table, c);
        let q = Query::select()
            .columns([e(Events::Box), e(Events::Kind), e(Events::Payload)])
            .from(Events::Table)
            .inner_join(
                Boxes::Table,
                Expr::col((Boxes::Table, Boxes::Id)).equals(e(Events::Box)),
            )
            .and_where(Expr::col((Boxes::Table, Boxes::Repo)).eq(repo.to_string()))
            .and_where(Expr::col(e(Events::Kind)).is_in(["credential_withheld", "egress_seen"]))
            .and_where(Expr::col(e(Events::Ts)).gte(Ts::from_millis(since).as_millis()))
            .order_by(e(Events::Id), Order::Asc)
            .limit(MOST)
            .take();
        let rows = self.reader()?.rows(q, |row| {
            let payload: String = row.get(2)?;
            Ok::<_, StoreError>((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                serde_json::from_str(&payload)?,
            ))
        })?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests;
