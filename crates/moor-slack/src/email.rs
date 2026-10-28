//! The verified email each Slack user's Enroll carries, so the same address in Slack and the
//! sign-in provider is one moor user.

use std::sync::Arc;

use moor_client::ClientError;
use moor_core::request::Request;
use moor_core::response::Response;
use moor_core::secret::Secret;
use moor_core::user::Email;

use crate::api::{Api, ApiError};
use crate::db::Db;
use crate::identity::Link;
use crate::Log;

/// How long a profile email read from Slack is trusted before it is read again.
const REFRESH_SECS: i64 = 24 * 60 * 60;

/// Reads Slack profile emails through `users.info`, remembering each in `slack.db`.
#[derive(Clone)]
pub(crate) struct Emails {
    /// Web API client.
    pub(crate) api: Arc<Api>,
    /// Bot token for `users.info`.
    pub(crate) bot_token: Arc<Secret>,
    /// Daemon log.
    pub(crate) log: Log,
    /// The current time in unix seconds.
    pub(crate) clock: fn() -> i64,
}

impl Emails {
    /// Runs `request` as Slack user `user` on `link`. An `enroll` without an email gets
    /// `user`'s profile email; every other request goes as it is.
    pub(crate) fn call(
        &self,
        link: &Link,
        db: &Db,
        user: &str,
        mut request: Request,
    ) -> Result<Response, ClientError> {
        if let Request::Enroll(enroll) = &mut request {
            if enroll.email.is_none() {
                enroll.email = self.of(db, user);
            }
        }
        link.request(user, request)
    }

    /// The email on Slack user `user`'s profile: the remembered one while it is under a day
    /// old, else a fresh read. A read that fails falls back to an older remembered one, and
    /// to none when there is not one; the reason goes to the log.
    fn of(&self, db: &Db, user: &str) -> Option<Email> {
        let now = (self.clock)();
        let known = db.user_email(user).unwrap_or_else(|e| {
            (self.log)(&format!("slack.db: read email of {user}: {e}"));
            None
        });
        if let Some((email, at)) = &known {
            if now.saturating_sub(*at) < REFRESH_SECS {
                return Email::parse(email).ok();
            }
        }
        let Some(email) = self.fetch(user) else {
            return known.and_then(|(email, _)| Email::parse(&email).ok());
        };
        if let Err(e) = db.set_user_email(user, email.as_str(), now) {
            (self.log)(&format!("slack.db: remember email of {user}: {e}"));
        }
        Some(email)
    }

    /// A fresh read of Slack user `user`'s profile email, or why there is none in the log.
    fn fetch(&self, user: &str) -> Option<Email> {
        match self.api.user_email(&self.bot_token, user) {
            Ok(Some(text)) => Email::parse(&text).map_err(|why| (self.log)(&why)).ok(),
            Ok(None) => None,
            Err(ApiError::Slack(code)) if code == "missing_scope" => {
                (self.log)(&format!(
                    "slack: users.info needs the bot scopes users:read and users:read.email; \
                     {user} is enrolled without an email"
                ));
                None
            }
            Err(e) => {
                (self.log)(&format!("slack: read email of {user}: {e}"));
                None
            }
        }
    }
}
