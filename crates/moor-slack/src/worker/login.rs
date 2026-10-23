//! The worker's half of `/moor login` and `/moor logout`: once the account takes a token it
//! goes into the controller's vault as the Slack user who pasted it, and a logout deletes it.

use super::Fx;
use crate::account::Account;
use crate::escape::escape;
use crate::login::{vault, TokenCheck, LINKED};

/// Checks the pasted token, stores it when the account takes it, and shows the outcome in the
/// modal. The token is not in the log line or in any text.
pub(super) fn link(fx: &mut Fx<'_>, check: &TokenCheck) {
    let checked = (fx.check_token)(check);
    let text = match checked {
        Err(text) => text,
        Ok(()) => match vault::store(
            &mut *fx.call,
            check.user.as_str(),
            check.account,
            &check.token,
        ) {
            Ok(()) => LINKED.to_owned(),
            Err(why) => format!("Could not store your token: {why}. Nothing was stored."),
        },
    };
    (fx.log)(&format!("login of {}: {text}", check.user.as_str()));
    (fx.modal)(&check.view_id, &text);
}

/// Deletes the user's secret and tells the user what happened.
pub(super) fn unlink(fx: &mut Fx<'_>, user: &str, account: Account, response_url: Option<&str>) {
    let label = account.label();
    let text = match vault::delete(&mut *fx.call, user, account) {
        Ok(true) => format!("Unlinked your {label} token."),
        Ok(false) => format!("You had no {label} token linked."),
        Err(why) => format!("Could not unlink your {label} token: {}", escape(&why)),
    };
    (fx.log)(&format!("logout of {user}: {text}"));
    match response_url {
        Some(url) => (fx.respond)(url, &text),
        None => (fx.log)("logout: the command carried no response_url"),
    }
}
