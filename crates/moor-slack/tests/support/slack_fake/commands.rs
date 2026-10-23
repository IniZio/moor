//! The `/moor` command side of the fake Slack: slash commands, modal submissions and the
//! answers the front gives through a command's `response_url`.

use serde_json::{json, Value};

use super::FakeSlack;

impl FakeSlack {
    /// Queues a `slash_commands` envelope: `user` runs `/moor <text>` in `channel`.
    pub(crate) fn send_slash(&self, envelope_id: &str, channel: &str, user: &str, text: &str) {
        let envelope = json!({
            "type": "slash_commands",
            "envelope_id": envelope_id,
            "payload": {
                "command": "/moor",
                "text": text,
                "user_id": user,
                "channel_id": channel,
                "trigger_id": format!("trigger-{envelope_id}"),
                "response_url": format!("{}/respond/{envelope_id}", self.api_base()),
            },
        });
        self.lock().outbox.push_back(envelope.to_string());
    }

    /// Queues an `interactive` envelope: `user` submits the login modal `view`, as opened,
    /// with `token` in its input.
    pub(crate) fn send_submission(&self, envelope_id: &str, user: &str, view: &Value, token: &str) {
        let envelope = json!({
            "type": "interactive",
            "envelope_id": envelope_id,
            "payload": {
                "type": "view_submission",
                "user": {"id": user},
                "view": {
                    "id": "V1",
                    "callback_id": view["callback_id"],
                    "private_metadata": view["private_metadata"],
                    "state": {"values": {"token": {"token": {
                        "type": "plain_text_input",
                        "value": token,
                    }}}},
                },
            },
        });
        self.lock().outbox.push_back(envelope.to_string());
    }

    /// The texts the front answered `/moor` commands with, through their `response_url`, as
    /// `(envelope id, text)`, in order.
    pub(crate) fn responses(&self) -> Vec<(String, String)> {
        self.requests()
            .iter()
            .filter_map(|r| {
                let id = r.path.strip_prefix("/respond/")?;
                Some((id.to_owned(), r.body["text"].as_str()?.to_owned()))
            })
            .collect()
    }

    /// Waits until some command answer exists and returns all of them.
    pub(crate) fn wait_responses(&self, n: usize) -> Vec<(String, String)> {
        Self::wait(|| Some(self.responses()).filter(|r| r.len() >= n))
            .unwrap_or_else(|| panic!("expected {n} answers, saw {:?}", self.responses()))
    }
}
