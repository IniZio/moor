//! Signing in to an OAuth MCP server from `/moor mcp add`: the add modal turns into a Sign in
//! link and a field for the address the browser ended on, and submitting it finishes the sign-in.

use moor_core::config::OAuthCallback;
use moor_core::request::{McpOAuthFinishReq, Request};
use serde_json::{json, Value};

use super::{name_ok, on_field, MCP_CALLBACK};
use crate::credentials::SlackUser;
use crate::login::vault::{send, Call};
use crate::socket::TokenSubmission;

/// The `private_metadata` prefix of the finish step; the server's name follows it.
pub(super) const FINISH_PREFIX: &str = "mcp-finish:";

/// Shown under the Sign in button.
const AFTER_APPROVE: &str =
    "After you approve, if the page doesn't load, copy its address and use Finish sign-in.";

/// The modal that asks the user to sign in to `name` at `authorize`, then paste the address
/// the sign-in ended on.
pub(super) fn sign_in(name: &str, authorize: &str) -> Value {
    json!({
        "type": "modal",
        "callback_id": MCP_CALLBACK,
        "private_metadata": format!("{FINISH_PREFIX}{name}"),
        "title": {"type": "plain_text", "text": "moor mcp"},
        "submit": {"type": "plain_text", "text": "Finish sign-in"},
        "close": {"type": "plain_text", "text": "Cancel"},
        "blocks": [
            {"type": "section", "text": {
                "type": "plain_text",
                "text": format!("Added `{name}`. Sign in to connect it."),
            }},
            {"type": "actions", "elements": [{
                "type": "button",
                "action_id": "sign_in",
                "style": "primary",
                "text": {"type": "plain_text", "text": "Sign in"},
                "url": authorize,
            }]},
            {"type": "context", "elements": [{"type": "plain_text", "text": AFTER_APPROVE}]},
            {
                "type": "input",
                "block_id": "address",
                "label": {"type": "plain_text", "text": "Address after sign-in"},
                "element": {"type": "plain_text_input", "action_id": "address"},
            },
        ],
    })
}

/// The ack that replaces the add modal with [`sign_in`].
pub(super) fn ask(name: &str, authorize: &str) -> Value {
    json!({"response_action": "update", "view": sign_in(name, authorize)})
}

/// What the authorization server sent back, read from the address the browser ended on.
fn redirect(address: &str) -> Result<McpOAuthFinishReq, String> {
    let OAuthCallback { state, code, iss } = OAuthCallback::from_url(address)?;
    Ok(McpOAuthFinishReq {
        identity: None,
        state,
        code,
        iss,
    })
}

/// Handles a submitted finish step as the Slack user: sends `mcp-oauth-finish` with what the
/// pasted address carries. A refusal stays on the field; success replaces the modal.
pub(super) fn finish(sub: &TokenSubmission, call: &mut Call<'_>) -> Value {
    let name = sub.account.strip_prefix(FINISH_PREFIX).unwrap_or_default();
    if SlackUser::parse(&sub.user).is_none() || !name_ok(name) {
        return super::update(&format!("{} Nothing was stored.", super::BAD_ID));
    }
    let request = match redirect(&super::field(sub, "address")) {
        Ok(request) => request,
        Err(why) => return on_field("address", &why),
    };
    match send(call, &sub.user, Request::McpOAuthFinish(request)) {
        Ok(_) => super::update(&format!("{name} connected")),
        Err(why) => on_field("address", &why),
    }
}
