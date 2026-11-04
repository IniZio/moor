//! Users, the identities linked to them, and each user's sealed secrets.

use moor_core::user::{Email, Identity, SecretName, UserId};
use moor_core::Secret;
use moor_schema::tables::{Identities, UserSecrets, Users};
use sea_query::{Expr, ExprTrait as _, OnConflict, Order, Query};

use super::{parsed, Store, StoreError};
use crate::store::db::{Db, DbExt as _, OptionalExtension, TransactionBehavior};
use crate::vault::Vault;

pub(super) mod front_leases;
pub(super) mod machine;
mod mcp;
pub(super) mod mcp_oauth;
mod profile;
pub(super) mod webhook;

/// The user `identity` is linked to, on any connection.
fn user_of_in(conn: &dyn Db, identity: &Identity) -> Result<Option<UserId>, StoreError> {
    let id: Option<i64> = conn
        .row(
            Query::select()
                .column(Identities::User)
                .from(Identities::Table)
                .and_where(Expr::col(Identities::Identity).eq(identity.as_str())),
            |r| r.get(0),
        )
        .optional()?;
    Ok(id.map(UserId::new))
}

impl Store {
    /// The user `identity` is linked to, creating one for it when it has none; true when this
    /// call created the user.
    ///
    /// With a verified `email`, an unknown identity joins the user that has the email, and a
    /// new user is created with it; a known identity's user gains the email when it has none
    /// and no other user holds it. No two users ever share an email.
    pub(crate) fn enroll(
        &self,
        identity: &Identity,
        email: Option<&Email>,
    ) -> Result<(UserId, bool), StoreError> {
        let now = self.now().as_millis();
        let email = email.map(Email::as_str);
        let mut writer = self.writer();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(user) = user_of_in(&tx, identity)? {
            if let Some(email) = email {
                let held = Query::select()
                    .column(Users::Id)
                    .from(Users::Table)
                    .and_where(Expr::col(Users::Email).eq(email))
                    .take();
                tx.run(
                    Query::update()
                        .table(Users::Table)
                        .value(Users::Email, email)
                        .and_where(Expr::col(Users::Id).eq(user.get()))
                        .and_where(Expr::col(Users::Email).is_null())
                        .and_where(Expr::exists(held).not()),
                )?;
            }
            tx.commit()?;
            return Ok((user, false));
        }
        let holder: Option<i64> = match email {
            Some(email) => tx
                .row(
                    Query::select()
                        .column(Users::Id)
                        .from(Users::Table)
                        .and_where(Expr::col(Users::Email).eq(email)),
                    |r| r.get(0),
                )
                .optional()?,
            None => None,
        };
        let (user, created) = if let Some(user) = holder {
            (user, false)
        } else {
            let user = tx.row(
                Query::insert()
                    .into_table(Users::Table)
                    .columns([Users::CreatedAt, Users::Email])
                    .values_panic([now.into(), email.into()])
                    .returning_col(Users::Id),
                |r| r.get(0),
            )?;
            (user, true)
        };
        tx.run(
            Query::insert()
                .into_table(Identities::Table)
                .columns([Identities::Identity, Identities::User, Identities::LinkedAt])
                .values_panic([identity.as_str().into(), user.into(), now.into()]),
        )?;
        tx.commit()?;
        drop(writer);
        Ok((UserId::new(user), created))
    }

    /// The user `identity` is linked to, if any.
    pub(crate) fn user_of(&self, identity: &Identity) -> Result<Option<UserId>, StoreError> {
        user_of_in(&self.reader()?, identity)
    }

    /// Every identity linked to `user`, in order.
    pub(crate) fn identities(&self, user: UserId) -> Result<Vec<Identity>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .column(Identities::Identity)
                .from(Identities::Table)
                .and_where(Expr::col(Identities::User).eq(user.get()))
                .order_by(Identities::Identity, Order::Asc),
            |r| {
                let raw: String = r.get(0)?;
                parsed("identity", &raw, Identity::parse)
            },
        )?;
        rows.collect()
    }

    /// Stores `sealed`, a value the vault encrypted, as `user`'s secret `name`.
    pub(crate) fn set_user_secret(
        &self,
        user: UserId,
        name: &SecretName,
        sealed: &[u8],
    ) -> Result<(), StoreError> {
        let now = self.now().as_millis();
        self.writer().run(
            Query::insert()
                .into_table(UserSecrets::Table)
                .columns([
                    UserSecrets::User,
                    UserSecrets::Name,
                    UserSecrets::Value,
                    UserSecrets::UpdatedAt,
                ])
                .values_panic([
                    user.get().into(),
                    name.as_str().into(),
                    sealed.into(),
                    now.into(),
                ])
                .on_conflict(
                    OnConflict::columns([UserSecrets::User, UserSecrets::Name])
                        .update_columns([UserSecrets::Value, UserSecrets::UpdatedAt])
                        .to_owned(),
                ),
        )?;
        Ok(())
    }

    /// Deletes `user`'s secret `name`; true when there was one.
    pub(crate) fn delete_user_secret(
        &self,
        user: UserId,
        name: &SecretName,
    ) -> Result<bool, StoreError> {
        let gone = self.writer().run(
            Query::delete()
                .from_table(UserSecrets::Table)
                .and_where(Expr::col(UserSecrets::User).eq(user.get()))
                .and_where(Expr::col(UserSecrets::Name).eq(name.as_str())),
        )?;
        Ok(gone == 1)
    }

    /// The names of `user`'s secrets, in order; never their values.
    pub(crate) fn user_secret_names(&self, user: UserId) -> Result<Vec<SecretName>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .column(UserSecrets::Name)
                .from(UserSecrets::Table)
                .and_where(Expr::col(UserSecrets::User).eq(user.get()))
                .order_by(UserSecrets::Name, Order::Asc),
            |r| {
                let raw: String = r.get(0)?;
                parsed("secret name", &raw, SecretName::parse)
            },
        )?;
        rows.collect()
    }

    /// `user`'s secret `name`, decrypted with `vault` on every call; nothing is cached, so a
    /// deleted secret is gone for the next caller.
    pub(crate) fn user_secret(
        &self,
        vault: &Vault,
        user: UserId,
        name: &SecretName,
    ) -> Result<Option<Secret>, StoreError> {
        let sealed: Option<Vec<u8>> = self
            .reader()?
            .row(
                Query::select()
                    .column(UserSecrets::Value)
                    .from(UserSecrets::Table)
                    .and_where(Expr::col(UserSecrets::User).eq(user.get()))
                    .and_where(Expr::col(UserSecrets::Name).eq(name.as_str())),
                |r| r.get(0),
            )
            .optional()?;
        Ok(sealed.map(|s| vault.open(user, name, &s)).transpose()?)
    }
}

#[cfg(test)]
mod tests;
