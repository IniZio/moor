//! Answering a slash command after its ack, through the `response_url` Slack sent with it.

use serde_json::json;

use super::{Api, ApiError};

/// Where Slack's response URLs live.
const SLACK_HOOKS: &str = "https://hooks.slack.com/";

impl Api {
    /// Shows `text` to the user who ran a slash command, through its `response_url`. A URL
    /// outside Slack's hooks host, or this client's own base host, is refused unsent, so a
    /// forged envelope cannot make the front post to an address of its choosing.
    pub(crate) fn respond(&self, response_url: &str, text: &str) -> Result<(), ApiError> {
        if !self.may_respond_to(response_url) {
            return Err(ApiError::Transport(format!(
                "response_url {response_url} is not a Slack hooks URL"
            )));
        }
        let body = json!({"response_type": "ephemeral", "text": text});
        let request = self
            .agent
            .post(response_url)
            .header("Content-Type", "application/json; charset=utf-8");
        let response = request
            .send(body.to_string())
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        match response.status().as_u16() {
            200..=299 => Ok(()),
            other => Err(ApiError::Transport(format!("HTTP {other}"))),
        }
    }

    /// Whether `url` is on Slack's hooks host or on the host of this client's base.
    fn may_respond_to(&self, url: &str) -> bool {
        let base_host = self
            .base
            .split_once("://")
            .map(|(scheme, rest)| (scheme, rest.split('/').next().unwrap_or(rest)));
        let own = base_host.map(|(scheme, host)| format!("{scheme}://{host}/"));
        url.starts_with(SLACK_HOOKS) || own.is_some_and(|own| url.starts_with(&own))
    }
}

#[cfg(test)]
mod tests {
    use super::Api;

    #[test]
    fn only_slack_hooks_and_the_own_base_host_are_answered() {
        let api = Api::new("https://slack.com/api").unwrap();
        assert!(api.may_respond_to("https://hooks.slack.com/commands/T1/1/abc"));
        assert!(api.may_respond_to("https://slack.com/x"));
        for url in [
            "https://hooks.slack.com.evil.example/commands",
            "http://hooks.slack.com/commands",
            "https://evil.example/https://hooks.slack.com/",
            "http://127.0.0.1:9/hook",
            "https://slack.com.evil.example/",
        ] {
            assert!(!api.may_respond_to(url), "{url}");
        }
        let loopback = Api::new("http://127.0.0.1:4000/api").unwrap();
        assert!(loopback.may_respond_to("http://127.0.0.1:4000/respond/1"));
        assert!(!loopback.may_respond_to("http://127.0.0.1:4001/respond/1"));
    }

    #[test]
    fn a_url_off_slack_is_refused_unsent() {
        let api = Api::new("http://127.0.0.1:9/api").unwrap();
        let err = api.respond("https://evil.example/x", "hi").unwrap_err();
        assert!(err.to_string().contains("not a Slack hooks URL"), "{err}");
    }
}
