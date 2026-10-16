//! Slack Web API calls made with the controller's tokens.

use std::sync::Arc;
use std::time::Duration;

use moor_core::secret::Secret;
use serde_json::{json, Value};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;

/// Largest response body read from Slack.
const MAX_BODY: u64 = 1024 * 1024;

/// Why a Web API call failed; never carries a token.
#[derive(Debug, thiserror::Error)]
pub(super) enum ApiError {
    /// The request did not complete or the reply was not understood.
    #[error("slack request failed: {0}")]
    Transport(String),
    /// Slack answered `ok: false` with this `error` code.
    #[error("slack rejected the call: {0}")]
    Slack(String),
    /// Slack answered HTTP 429; the caller should back off.
    #[error("slack rate limited the call")]
    RateLimited,
}

/// A Slack Web API client rooted at one base URL.
pub(super) struct Api {
    /// Base URL without a trailing slash.
    base: String,
    /// HTTP client shared by every call.
    agent: Agent,
}

impl Api {
    /// A client for `base`, such as `https://slack.com/api`.
    pub(super) fn new(base: &str) -> Result<Self, ApiError> {
        if !(base.starts_with("https://") || base.starts_with("http://")) {
            return Err(ApiError::Transport("base URL must be http or https".into()));
        }
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build();
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            agent: Agent::new_with_config(config),
        })
    }

    /// Asks Slack for a one-shot Socket Mode WebSocket URL.
    pub(super) fn open_connection(&self, app_token: &Secret) -> Result<String, ApiError> {
        let url = format!("{}/apps.connections.open", self.base);
        let request = self
            .agent
            .post(&url)
            .header("Authorization", bearer(app_token)?);
        let reply = read_reply(request.send_empty())?;
        reply
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| ApiError::Transport("reply has no url".into()))
    }

    /// Posts `text` into the thread `thread_ts` of `channel`; returns the new message `ts`.
    pub(super) fn post_message(
        &self,
        bot_token: &Secret,
        channel: &str,
        thread_ts: &str,
        text: &str,
    ) -> Result<String, ApiError> {
        let body = json!({"channel": channel, "thread_ts": thread_ts, "text": text});
        reply_ts(&self.call(bot_token, "chat.postMessage", &body)?)
    }

    /// Replaces the text of the message `ts` in `channel`.
    pub(super) fn update_message(
        &self,
        bot_token: &Secret,
        channel: &str,
        ts: &str,
        text: &str,
    ) -> Result<(), ApiError> {
        let body = json!({"channel": channel, "ts": ts, "text": text});
        self.call(bot_token, "chat.update", &body).map(|_| ())
    }

    /// Posts `text` with Block Kit `blocks` into thread `thread_ts`; returns the new message ts.
    pub(super) fn post_blocks(
        &self,
        bot_token: &Secret,
        channel: &str,
        thread_ts: &str,
        text: &str,
        blocks: &Value,
    ) -> Result<String, ApiError> {
        let body =
            json!({"channel": channel, "thread_ts": thread_ts, "text": text, "blocks": blocks});
        reply_ts(&self.call(bot_token, "chat.postMessage", &body)?)
    }

    /// Replaces text and blocks of message `ts`.
    pub(super) fn update_blocks(
        &self,
        bot_token: &Secret,
        channel: &str,
        ts: &str,
        text: &str,
        blocks: &Value,
    ) -> Result<(), ApiError> {
        let body = json!({"channel": channel, "ts": ts, "text": text, "blocks": blocks});
        self.call(bot_token, "chat.update", &body).map(|_| ())
    }

    /// chat.postEphemeral: shows `text` to `user` only, inside thread `thread_ts`.
    pub(super) fn post_ephemeral(
        &self,
        bot_token: &Secret,
        channel: &str,
        user: &str,
        thread_ts: &str,
        text: &str,
    ) -> Result<(), ApiError> {
        let body = json!({"channel": channel, "user": user, "thread_ts": thread_ts, "text": text});
        self.call(bot_token, "chat.postEphemeral", &body)
            .map(|_| ())
    }

    /// Sends the JSON `body` to Web API `method` with the bot token.
    fn call(&self, bot_token: &Secret, method: &str, body: &Value) -> Result<Value, ApiError> {
        let url = format!("{}/{method}", self.base);
        let request = self
            .agent
            .post(&url)
            .header("Authorization", bearer(bot_token)?)
            .header("Content-Type", "application/json; charset=utf-8");
        read_reply(request.send(body.to_string()))
    }
}

/// The message `ts` in a posting reply.
fn reply_ts(reply: &Value) -> Result<String, ApiError> {
    reply
        .get("ts")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ApiError::Slack("reply has no ts".into()))
}

/// The `Authorization` header value for `token`.
fn bearer(token: &Secret) -> Result<String, ApiError> {
    let text = std::str::from_utf8(token.expose())
        .map_err(|_| ApiError::Transport("token is not valid text".into()))?;
    Ok(format!("Bearer {text}"))
}

/// Parses a Slack reply, mapping `ok: false` to [`ApiError::Slack`].
fn read_reply(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<Value, ApiError> {
    let mut response = response.map_err(|e| ApiError::Transport(e.to_string()))?;
    let status = response.status();
    if status.as_u16() == 429 {
        return Err(ApiError::RateLimited);
    }
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_string()
        .map_err(|e| ApiError::Transport(e.to_string()))?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|_| ApiError::Transport(format!("unreadable reply (HTTP {status})")))?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(value);
    }
    Err(value.get("error").and_then(Value::as_str).map_or_else(
        || ApiError::Transport(format!("unexpected reply (HTTP {status})")),
        |code| ApiError::Slack(code.to_owned()),
    ))
}

#[cfg(test)]
mod tests;
