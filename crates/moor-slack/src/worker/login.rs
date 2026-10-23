//! The worker's half of `/moor login` and `/moor logout`: once the account takes a token it
//! goes into the controller's vault as the Slack user who pasted it, and a logout deletes it.

use super::Fx;
use crate::account::Account;
use crate::escape::escape;
use crate::login::{vault, TokenCheck, LINKED};

/// Checks each pasted token, stores the ones its account takes, and shows every outcome in the
/// one modal. A failed token does not stop the others. With several tokens each line starts
/// with its account. No token is in the log line or in any text.
pub(super) fn link(fx: &mut Fx<'_>, checks: &[TokenCheck]) {
    let lines: Vec<String> = checks
        .iter()
        .map(|check| {
            let text = match (fx.check_token)(check) {
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
            match checks.len() {
                1 => text,
                _ => format!("{}: {text}", check.account.label()),
            }
        })
        .collect();
    let text = lines.join("\n");
    if let Some(first) = checks.first() {
        (fx.log)(&format!("login of {}: {text}", first.user.as_str()));
        (fx.modal)(&first.view_id, &text);
    }
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
