//! Who the bot is, and the metadata of a message it posted.

use moor_core::secret::Secret;
use serde_json::{json, Value};

use super::{bearer, form_encode, read_reply, Api, ApiError};

/// The bot token's owner, from `auth.test`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BotIdentity {
    /// The workspace.
    pub(crate) team: String,
    /// The bot's user id.
    pub(crate) user: Option<String>,
    /// The bot's `bot_id`, which its own posts carry.
    pub(crate) bot: Option<String>,
}

impl Api {
    /// Who the bot token belongs to, as `auth.test` reports it.
    pub(crate) fn auth_test(&self, bot_token: &Secret) -> Result<BotIdentity, ApiError> {
        let reply = self.call(bot_token, "auth.test", &json!({}))?;
        let text = |key: &str| reply.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(BotIdentity {
            team: text("team_id")
                .ok_or_else(|| ApiError::Transport("reply has no team_id".into()))?,
            user: text("user_id"),
            bot: text("bot_id"),
        })
    }

    /// The profile email of Slack user `user`, from `users.info`; none when the profile has
    /// none, as a bot user's. Needs the bot scopes `users:read` and `users:read.email`.
    pub(crate) fn user_email(
        &self,
        bot_token: &Secret,
        user: &str,
    ) -> Result<Option<String>, ApiError> {
        let url = format!("{}/users.info", self.base);
        let request = self
            .agent
            .post(&url)
            .header("Authorization", bearer(bot_token)?)
            .header("Content-Type", "application/x-www-form-urlencoded");
        let reply = read_reply(request.send(format!("user={}", form_encode(user))))?;
        Ok(reply
            .pointer("/user/profile/email")
            .and_then(Value::as_str)
            .filter(|email| !email.is_empty())
            .map(str::to_owned))
    }

    /// The metadata of the message `ts` in thread `root` of `channel`, if it has any.
    pub(crate) fn message_metadata(
        &self,
        bot_token: &Secret,
        channel: &str,
        root: &str,
        ts: &str,
    ) -> Result<Option<Value>, ApiError> {
        let url = format!("{}/conversations.replies", self.base);
        let request = self
            .agent
            .post(&url)
            .header("Authorization", bearer(bot_token)?)
            .header("Content-Type", "application/x-www-form-urlencoded");
        let body = format!(
            "channel={}&ts={}&oldest={ts_}&latest={ts_}&inclusive=true&include_all_metadata=true",
            form_encode(channel),
            form_encode(root),
            ts_ = form_encode(ts),
        );
        let reply = read_reply(request.send(body))?;
        let messages = reply.get("messages").and_then(Value::as_array);
        Ok(messages
            .into_iter()
            .flatten()
            .find(|m| m.get("ts").and_then(Value::as_str) == Some(ts))
            .and_then(|m| m.get("metadata"))
            .cloned())
    }
}
