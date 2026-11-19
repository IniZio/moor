//! The newest profile failure on any of a user's Boxes.

use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_schema::tables::{Boxes, Events};
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::db::{DbExt as _, OptionalExtension as _};
use super::{Store, StoreError};

/// One `profile_failed` event with the Box it happened on.
#[derive(Debug, PartialEq)]
pub(crate) struct ProfileFailedRow {
    /// The Box's name.
    pub(crate) box_name: String,
    /// The event's fields without `kind`.
    pub(crate) payload: serde_json::Value,
    /// When it was recorded.
    pub(crate) at: Ts,
}

impl Store {
    /// The newest `profile_failed` event on a Box of `user`, if any Box has had one.
    pub(crate) fn newest_profile_failed(
        &self,
        user: UserId,
    ) -> Result<Option<ProfileFailedRow>, StoreError> {
        let row: Option<(String, String, i64)> = self
            .reader()?
            .row(
                Query::select()
                    .column((Boxes::Table, Boxes::Name))
                    .column((Events::Table, Events::Payload))
                    .column((Events::Table, Events::Ts))
                    .from(Events::Table)
                    .inner_join(
                        Boxes::Table,
                        Expr::col((Boxes::Table, Boxes::Id)).equals((Events::Table, Events::Box)),
                    )
                    .and_where(Expr::col((Boxes::Table, Boxes::User)).eq(user.get()))
                    .and_where(Expr::col((Events::Table, Events::Kind)).eq("profile_failed"))
                    .order_by((Events::Table, Events::Id), Order::Desc)
                    .limit(1),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        row.map(|(box_name, payload, at)| {
            Ok(ProfileFailedRow {
                box_name,
                payload: serde_json::from_str(&payload)?,
                at: Ts::from_millis(at),
            })
        })
        .transpose()
    }
}
