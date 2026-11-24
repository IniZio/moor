//! The places a user has had, for the keeper that gives each user one.

use moor_core::user::UserId;
use moor_schema::tables::Boxes;
use sea_query::{Expr, ExprTrait as _, Order};

use super::reads::query_boxes;
use super::{BoxRow, Store, StoreError};

impl Store {
    /// The newest place `user` has had, in any lifecycle: closed and rejected ones too.
    pub(crate) fn newest_place_of(&self, user: UserId) -> Result<Option<BoxRow>, StoreError> {
        let b = |c| (Boxes::Table, c);
        let filter = Expr::col(b(Boxes::Kind))
            .eq("place")
            .and(Expr::col(b(Boxes::User)).eq(user.get()));
        let rows = query_boxes(&self.reader()?, filter, Order::Desc, Some(1))?;
        Ok(rows.into_iter().next())
    }
}
