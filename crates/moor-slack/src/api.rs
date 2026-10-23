//! Slack Web API calls made with the controller's tokens.

use std::fmt::Write as _;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::secret::Secret;
use serde_json::{json, Value};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::{Agent, SendBody};

/// Largest response body read from Slack.
const MAX_BODY: u64 = 1024 * 1024;
/// Time limit of a JSON call, and the base time limit of a file upload.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Slowest transfer rate a file upload may run at, in bytes per second, before it times out.
const UPLOAD_FLOOR_RATE: u64 = 1024 * 1024;

/// Where Slack takes one file's bytes, and the id that later shares the file.
pub(super) struct UploadUrl {
    /// URL the raw bytes go to.
    url: String,
    /// Slack's id for the file.
    pub(super) file_id: String,
}

/// Why a Web API call failed; never carries a token.
#[derive(Debug, thiserror::Error)]
pub(super) enum ApiError {
    /// The request did not complete or the reply was not understood.
    #[error("slack request failed: {0}")]
    Transport(String),
    /// Slack answered `ok: false` with this `error` code.
    #[error("slack rejected the call: {0}")]
    Slack(String),
    /// Slack answered HTTP 429; the caller should back off, for as long as Slack asked when
    /// it said.
    #[error("slack rate limited the call")]
    RateLimited(Option<Duration>),
}

/// A Slack Web API client rooted at one base URL.
pub(super) struct Api {
    /// Base URL without a trailing slash.
    base: String,
    /// HTTP client shared by every call.
    agent: Agent,
    /// Whether the missing `files:read` scope was already reported.
    files_read_reported: AtomicBool,
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
            .timeout_global(Some(CALL_TIMEOUT))
            .build();
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            agent: Agent::new_with_config(config),
            files_read_reported: AtomicBool::new(false),
        })
    }

    /// Whether this is the first report of the missing `files:read` scope.
    pub(super) fn first_missing_files_read(&self) -> bool {
        !self.files_read_reported.swap(true, Ordering::Relaxed)
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

    /// The id of the workspace the bot token belongs to.
    pub(super) fn team_id(&self, bot_token: &Secret) -> Result<String, ApiError> {
        let reply = self.call(bot_token, "auth.test", &json!({}))?;
        reply
            .get("team_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| ApiError::Transport("reply has no team_id".into()))
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

    /// Opens the modal `view` for the user whose action carried `trigger_id`.
    pub(super) fn open_view(
        &self,
        bot_token: &Secret,
        trigger_id: &str,
        view: &Value,
    ) -> Result<(), ApiError> {
        let body = json!({"trigger_id": trigger_id, "view": view});
        self.call(bot_token, "views.open", &body).map(|_| ())
    }

    /// Replaces the open modal `view_id` with `view`.
    pub(super) fn update_view(
        &self,
        bot_token: &Secret,
        view_id: &str,
        view: &Value,
    ) -> Result<(), ApiError> {
        let body = json!({"view_id": view_id, "view": view});
        self.call(bot_token, "views.update", &body).map(|_| ())
    }

    /// Asks Slack where to send a file named `name` of `len` bytes.
    pub(super) fn upload_url(
        &self,
        bot_token: &Secret,
        name: &str,
        len: u64,
    ) -> Result<UploadUrl, ApiError> {
        let url = format!("{}/files.getUploadURLExternal", self.base);
        let form = format!("filename={}&length={len}", form_encode(name));
        let request = self
            .agent
            .post(&url)
            .header("Authorization", bearer(bot_token)?)
            .header("Content-Type", "application/x-www-form-urlencoded");
        let reply = read_reply(request.send(form))?;
        let field = |name: &str| {
            reply
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| ApiError::Transport(format!("reply has no {name}")))
        };
        Ok(UploadUrl {
            url: field("upload_url")?,
            file_id: field("file_id")?,
        })
    }

    /// POSTs exactly `len` bytes read from `body` to `to`, without the bot token. The time
    /// limit grows with `len`.
    pub(super) fn send_file(
        &self,
        to: &UploadUrl,
        len: u64,
        body: &mut dyn Read,
    ) -> Result<(), ApiError> {
        let sent = self
            .agent
            .post(&to.url)
            .config()
            .timeout_global(Some(
                CALL_TIMEOUT + Duration::from_secs(len / UPLOAD_FLOOR_RATE),
            ))
            .build()
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", len)
            .send(SendBody::from_reader(body))
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        let status = sent.status();
        if !status.is_success() {
            return Err(ApiError::Transport(format!(
                "file upload refused (HTTP {status})"
            )));
        }
        Ok(())
    }

    /// Shares the sent file `to` as `title` in thread `thread_ts` of `channel`.
    pub(super) fn complete_upload(
        &self,
        bot_token: &Secret,
        channel: &str,
        thread_ts: &str,
        to: &UploadUrl,
        title: &str,
    ) -> Result<(), ApiError> {
        let body = json!({
            "files": [{"id": to.file_id, "title": title}],
            "channel_id": channel,
            "thread_ts": thread_ts,
        });
        self.call(bot_token, "files.completeUploadExternal", &body)
            .map(|_| ())
    }

    /// Whether file `file_id` is shared in thread `thread_ts` of `channel`, by `files.info`.
    pub(super) fn file_shared(
        &self,
        bot_token: &Secret,
        file_id: &str,
        channel: &str,
        thread_ts: &str,
    ) -> Result<bool, ApiError> {
        let url = format!("{}/files.info", self.base);
        let request = self
            .agent
            .post(&url)
            .header("Authorization", bearer(bot_token)?)
            .header("Content-Type", "application/x-www-form-urlencoded");
        let reply = read_reply(request.send(format!("file={}", form_encode(file_id))))?;
        let shares = reply.pointer("/file/shares");
        Ok(["public", "private"].iter().any(|kind| {
            shares
                .and_then(|s| s.get(kind))
                .and_then(|k| k.get(channel))
                .and_then(Value::as_array)
                .is_some_and(|list| {
                    list.iter()
                        .any(|m| m.get("thread_ts").and_then(Value::as_str) == Some(thread_ts))
                })
        }))
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

/// Percent-encodes `text` for an `application/x-www-form-urlencoded` value.
fn form_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
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
        let after = response.headers().get("retry-after");
        let secs = after.and_then(|v| v.to_str().ok()?.trim().parse().ok());
        return Err(ApiError::RateLimited(secs.map(Duration::from_secs)));
    }
    if status.is_server_error() {
        return Err(ApiError::Transport(format!("HTTP {status}")));
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
