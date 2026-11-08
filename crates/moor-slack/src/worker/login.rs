//! The worker's half of `/moor logout`, which deletes a linked token, and of the `/moor mcp`,
//! `machine` and `profile` answers.

use super::Fx;
use crate::account::Account;
use crate::escape::escape;
use crate::login::vault;
use crate::machine::MachineJob;
use crate::mcp::McpJob;
use crate::profile::ProfileJob;

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

/// Answers `/moor mcp list` or `/moor mcp remove` to the user alone.
pub(super) fn mcp(fx: &mut Fx<'_>, job: &McpJob) {
    let text = match &job.name {
        None => crate::mcp::listing(&mut *fx.call, &job.user),
        Some(name) => crate::mcp::removal(&mut *fx.call, &job.user, name),
    };
    match job.response_url.as_deref() {
        Some(url) => (fx.respond)(url, &text),
        None => (fx.log)("mcp: the command carried no response_url"),
    }
}

/// Answers `/moor machine list` or `/moor machine remove` to the user alone.
pub(super) fn machine(fx: &mut Fx<'_>, job: &MachineJob) {
    let text = match &job.name {
        None => crate::machine::listing(&mut *fx.call, &job.user),
        Some(name) => crate::machine::removal(&mut *fx.call, &job.user, name),
    };
    match job.response_url.as_deref() {
        Some(url) => (fx.respond)(url, &text),
        None => (fx.log)("machine: the command carried no response_url"),
    }
}

/// Answers `/moor profile` to the user alone.
pub(super) fn profile(fx: &mut Fx<'_>, job: &ProfileJob) {
    let text = crate::profile::answer(&mut *fx.call, job);
    match job.response_url.as_deref() {
        Some(url) => (fx.respond)(url, &text),
        None => (fx.log)("profile: the command carried no response_url"),
    }
}
