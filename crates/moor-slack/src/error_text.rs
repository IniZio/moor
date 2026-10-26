//! What Slack says about a controller error. The controller words its messages for the CLI,
//! often ending in a `moor` command, so a kind that has a Slack action is read from the
//! error's detail and worded here; any other error keeps the controller's message.

use moor_core::response::{ErrorDetail, Response};

/// The text of a failed reply.
pub(crate) fn error_text(reply: Response) -> String {
    let Some(error) = reply.error else {
        return format!("{} failed", reply.cmd);
    };
    match error.detail {
        Some(ErrorDetail::AgentGone) => {
            "The agent of this Box has ended. Restart the agent to continue.".to_owned()
        }
        Some(ErrorDetail::FirstRunScreen) => {
            "The agent stopped at a first-run screen it should have skipped, which is a bug in \
             the tool. Close this Box and start a new one."
                .to_owned()
        }
        Some(ErrorDetail::RelaunchCapped) => {
            "The agent keeps ending soon after it is relaunched, so it will not be relaunched \
             again on its own. Find out why it ends, then restart the agent."
                .to_owned()
        }
        Some(ErrorDetail::LoginRequired) => {
            "A token you have not linked is needed. Link it with `/moor login`, then try again."
                .to_owned()
        }
        Some(ErrorDetail::ProvisioningDead) => {
            "This Box never finished starting. Close it and start a new one.".to_owned()
        }
        Some(ErrorDetail::CloseRefused(_)) | None => error.message,
    }
}

#[cfg(test)]
mod tests;
