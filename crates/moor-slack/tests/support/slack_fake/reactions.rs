//! Reactions and scripted posts in the fake Slack.

use serde_json::{json, Value};

use super::{message_ts, FakeSlack, State, BOT_TOKEN};

/// A `reactions.add` or `reactions.remove` the controller made with a valid bot token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reaction {
    /// True for `reactions.add`, false for `reactions.remove`.
    pub(crate) added: bool,
    /// Channel id.
    pub(crate) channel: String,
    /// Ts of the message reacted to.
    pub(crate) ts: String,
    /// Emoji name.
    pub(crate) name: String,
}

impl State {
    /// Answers a `chat.postMessage` for `body`: a refusal or scripted answer when one is
    /// queued, else a new message.
    pub(super) fn post(&mut self, body: &Value) -> (u16, Value) {
        if !body["blocks"].is_null() && self.refused_block_posts.0 > 0 {
            self.refused_block_posts.0 -= 1;
            return (
                200,
                json!({"ok": false, "error": self.refused_block_posts.1}),
            );
        }
        if let Some(answer) = self.post_answers.pop_front() {
            return answer;
        }
        self.posted += 1;
        (200, json!({"ok": true, "ts": message_ts(self.posted)}))
    }

    /// Answers a `reactions.add` (`add`) or `reactions.remove` for `body` the way Slack does.
    pub(super) fn react(&mut self, add: bool, body: &Value) -> Value {
        let text = |key: &str| body[key].as_str().unwrap_or_default().to_owned();
        let key = (text("channel"), text("timestamp"), text("name"));
        let there = self.reactions.contains(&key);
        if let Some(code) = self.reaction_error {
            return json!({"ok": false, "error": code});
        }
        match (add, there) {
            (true, true) => return json!({"ok": false, "error": "already_reacted"}),
            (false, false) => return json!({"ok": false, "error": "no_reaction"}),
            (true, false) => self.reactions.push(key),
            (false, true) => self.reactions.retain(|r| *r != key),
        }
        json!({"ok": true})
    }
}

impl FakeSlack {
    /// Puts reaction `name` on message `ts` of `channel`, as if an earlier run had.
    pub(crate) fn leave_reaction(&self, channel: &str, ts: &str, name: &str) {
        let key = (channel.to_owned(), ts.to_owned(), name.to_owned());
        self.lock().reactions.push(key);
    }

    /// Waits until no reaction is on any message; fails after the deadline.
    pub(crate) fn wait_no_reactions(&self) {
        Self::wait(|| self.reactions_now().is_empty().then_some(()))
            .unwrap_or_else(|| panic!("reactions left: {:?}", self.reactions_now()));
    }

    /// The reactions now on messages, as `(channel, ts, name)`.
    pub(crate) fn reactions_now(&self) -> Vec<(String, String, String)> {
        self.lock().reactions.clone()
    }

    /// Every reaction call made with the bot token, in order, whether Slack took it or not.
    pub(crate) fn reaction_calls(&self) -> Vec<Reaction> {
        self.requests()
            .iter()
            .filter(|r| {
                matches!(r.path.as_str(), "/reactions.add" | "/reactions.remove")
                    && r.auth == format!("Bearer {BOT_TOKEN}")
            })
            .map(|r| {
                let text = |key: &str| r.body[key].as_str().unwrap_or_default().to_owned();
                Reaction {
                    added: r.path == "/reactions.add",
                    channel: text("channel"),
                    ts: text("timestamp"),
                    name: text("name"),
                }
            })
            .collect()
    }

    /// Waits until at least `n` reaction calls were made and returns all of them.
    pub(crate) fn wait_reactions(&self, n: usize) -> Vec<Reaction> {
        Self::wait(|| Some(self.reaction_calls()).filter(|c| c.len() >= n)).unwrap_or_else(|| {
            panic!(
                "expected {n} reaction calls, saw {:?}",
                self.reaction_calls()
            )
        })
    }
}
