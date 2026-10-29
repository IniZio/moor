//! `moor-herdr login`: signs in and saves the credentials a dial refreshes from.

use moor_client::oidc;

pub use oidc::{Console, Ui, REDIRECT_PORT};

use super::{Credentials, Store, DEFAULT_LIFETIME};

/// Signs in to `server` and saves the credentials in `store`. `port` is the loopback port
/// for the browser's return; 0 takes any free one. `now` is unix seconds.
///
/// # Errors
/// The sign-in fails, grants no refresh token, or the credentials cannot be saved.
pub fn run(
    server: &str,
    port: u16,
    store: &Store,
    ui: &mut dyn Ui,
    now: i64,
) -> Result<(), String> {
    let signed = oidc::sign_in(server, port, ui)?;
    let refresh_token = signed.refresh_token.ok_or_else(|| {
        "the sign-in returned no refresh token; offline access is needed".to_owned()
    })?;
    let lifetime = signed.expires_in.map_or(DEFAULT_LIFETIME, |d| {
        i64::try_from(d.as_secs()).unwrap_or(DEFAULT_LIFETIME)
    });
    store
        .write(&Credentials {
            server: signed.server,
            issuer: signed.issuer,
            client_id: signed.client_id,
            token_endpoint: signed.token_endpoint,
            refresh_token,
            access_token: signed.access_token,
            expires_at: now + lifetime,
        })
        .map_err(|e| format!("cannot save the credentials: {e}"))
}
