//! Checking a session with the issuer: its refresh token is traded for new tokens, so a person
//! the issuer removed or disabled is refused and loses the portal within [`EVERY`].

use std::time::{Duration, SystemTime};

use oauth2::basic::BasicClient;
use oauth2::{ClientId, RefreshToken, RequestTokenError, TokenResponse, TokenUrl};

use super::{seal, Login, Session};
use crate::{log, Verifier};

/// How long a check holds: past it, the session opens only for `/auth/check` and `/auth/renew`.
pub(super) const EVERY: Duration = Duration::from_mins(5);

/// How long after the issuer's last yes a session may go on while the issuer cannot be reached.
const GRACE: Duration = Duration::from_mins(30);

/// How long a session goes on unchecked when the issuer cannot be reached, before it asks again.
const RETRY: Duration = Duration::from_mins(1);

impl Login {
    /// Asks the issuer whether `session`'s person may still sign in, by trading its refresh token,
    /// and stores the answer in `session`. A session the issuer gave no refresh token passes.
    /// While the issuer cannot be reached, a session passes for [`RETRY`] at a time, until
    /// [`GRACE`] after the issuer's last yes.
    ///
    /// # Errors
    /// Why the session must end: the issuer refused the token, named another person, or has been
    /// out of reach past [`GRACE`].
    pub(crate) fn recheck(
        &self,
        verifier: &Verifier,
        session: &mut Session,
        now: SystemTime,
    ) -> Result<(), String> {
        let Some(refresh) = session.refresh.clone() else {
            return Ok(());
        };
        let at = seal::unix(now);
        match self.trade(verifier, refresh) {
            Ok((person, next)) if person.identity == session.person.identity => {
                session.refresh = next.or_else(|| session.refresh.take());
                session.due = at.saturating_add(EVERY.as_secs());
                session.vouched = at;
                Ok(())
            }
            Ok(_) => Err("the issuer named another person".to_owned()),
            Err(Answer::Refused(why)) => Err(why),
            Err(Answer::Unreachable(why)) => {
                let silent = at.saturating_sub(session.vouched);
                if silent >= GRACE.as_secs() {
                    return Err(format!("the issuer has not answered for {silent}s: {why}"));
                }
                log(&format!(
                    "a session check could not reach the issuer: {why}"
                ));
                session.due = at.saturating_add(RETRY.as_secs());
                Ok(())
            }
        }
    }

    /// The person the issuer's new access token names, and the refresh token to keep, if it
    /// gave a new one.
    fn trade(
        &self,
        verifier: &Verifier,
        refresh: String,
    ) -> Result<(crate::Person, Option<String>), Answer> {
        let endpoints = self.endpoints(verifier).map_err(Answer::Unreachable)?;
        let url = TokenUrl::new(endpoints.token).map_err(|e| Answer::Unreachable(e.to_string()))?;
        let token = BasicClient::new(ClientId::new(verifier.client_id().to_owned()))
            .set_token_uri(url)
            .exchange_refresh_token(&RefreshToken::new(refresh))
            .request(&self.http)
            .map_err(|e| match e {
                RequestTokenError::ServerResponse(r) => {
                    Answer::Refused(format!("the issuer refused: {r}"))
                }
                other => Answer::Unreachable(other.to_string()),
            })?;
        let person = verifier
            .verify(token.access_token().secret())
            .map_err(Answer::Refused)?;
        Ok((person, token.refresh_token().map(|r| r.secret().clone())))
    }
}

/// Why a refresh did not give a person.
enum Answer {
    /// The issuer answered no.
    Refused(String),
    /// The issuer did not answer.
    Unreachable(String),
}
