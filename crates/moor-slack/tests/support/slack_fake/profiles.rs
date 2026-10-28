//! User profiles in the fake Slack.

use serde_json::{json, Value};

use super::{FakeSlack, State};

impl State {
    /// The reply to a `users.info`, `reactions.add` or `reactions.remove` call at `path`.
    pub(super) fn state_reply(&mut self, path: &str, body: &Value) -> Value {
        if path == "/users.info" {
            self.profile(body)
        } else {
            self.react(path.ends_with("add"), body)
        }
    }

    /// The `users.info` reply for the user `body` names.
    fn profile(&self, body: &Value) -> Value {
        let user = body["user"].as_str().unwrap_or_default();
        match (self.users_info_error, self.profile_emails.get(user)) {
            (Some(code), _) => json!({"ok": false, "error": code}),
            (None, Some(email)) => {
                json!({"ok": true, "user": {"id": user, "profile": {"email": email}}})
            }
            (None, None) => json!({"ok": true, "user": {"id": user, "profile": {}}}),
        }
    }
}

impl FakeSlack {
    /// Gives Slack user `user` the profile email `email`.
    pub(crate) fn set_email(&self, user: &str, email: &str) {
        self.lock()
            .profile_emails
            .insert(user.to_owned(), email.to_owned());
    }

    /// Answers every later `users.info` with `ok: false` and this error code.
    pub(crate) fn refuse_users_info(&self, code: &'static str) {
        self.lock().users_info_error = Some(code);
    }

    /// How many `users.info` calls the fake has answered.
    pub(crate) fn users_info_calls(&self) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.path == "/users.info")
            .count()
    }
}
