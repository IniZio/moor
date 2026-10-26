//! The worker's half of `/moor login` and `/moor logout`: once the account takes a token it
//! goes into the controller's vault as the Slack user who pasted it, and a logout deletes it.

use super::Fx;
use crate::account::Account;
use crate::escape::escape;
use crate::login::{vault, TokenCheck, LINKED};

/// Checks each pasted token, stores the ones its account takes, and shows every outcome in the
/// one modal. A failed token does not stop the others. When the form took both accounts, each
/// line names its account. No token is in the log line or in any text.
pub(super) fn link(fx: &mut Fx<'_>, checks: &[TokenCheck]) {
    let lines: Vec<String> = checks
        .iter()
        .map(|check| {
            let label = check.account.label();
            let stored = match (fx.check_token)(check) {
                Err(text) => Err(text),
                Ok(()) => vault::store(
                    &mut *fx.call,
                    check.user.as_str(),
                    check.account,
                    &check.token,
                )
                .map_err(|why| format!("Could not store your token: {why}. Nothing was stored.")),
            };
            match (stored, check.named) {
                (Ok(()), false) => LINKED.to_owned(),
                (Ok(()), true) => format!("Linked {label}."),
                (Err(text), true) if !text.starts_with(label) => format!("{label}: {text}"),
                (Err(text), _) => text,
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

/// Tells the user which MCP servers they have; the answer is theirs alone.
pub(super) fn mcp_list(fx: &mut Fx<'_>, user: &str, response_url: Option<&str>) {
    let text = crate::mcp::listing(&mut *fx.call, user);
    answer(fx, "mcp list", &text, response_url);
}

/// Removes the user's MCP server `name` and tells them.
pub(super) fn mcp_remove(fx: &mut Fx<'_>, user: &str, name: &str, response_url: Option<&str>) {
    let text = crate::mcp::removal(&mut *fx.call, user, name);
    answer(fx, "mcp remove", &text, response_url);
}

/// Sends `text` through the command's `response_url`.
fn answer(fx: &mut Fx<'_>, what: &str, text: &str, response_url: Option<&str>) {
    match response_url {
        Some(url) => (fx.respond)(url, text),
        None => (fx.log)(&format!("{what}: the command carried no response_url")),
    }
}
