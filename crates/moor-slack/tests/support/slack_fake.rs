//! An in-process fake of the Slack Web API and Socket Mode, for driving the controller's Slack app.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::Message;

use request::read_request;

mod commands;
mod profiles;
mod reactions;
mod request;

/// App-level token the fake accepts for opening a connection.
pub(crate) const APP_TOKEN: &str = "xapp-test";
/// The team id `auth.test` reports, and so the team of every identity the front asserts.
pub(crate) const TEAM: &str = "T1";
/// Bot token the fake accepts for posting messages.
pub(crate) const BOT_TOKEN: &str = "xoxb-test";

/// How long the `wait_*` helpers poll before giving up.
const DEADLINE: Duration = Duration::from_secs(10);
/// Pause between polls.
const POLL: Duration = Duration::from_millis(20);

/// One Web API request the fake received.
#[derive(Clone, Debug)]
pub(crate) struct Recorded {
    /// Request path, such as `/chat.postMessage`.
    pub(crate) path: String,
    /// The `Authorization` header value, empty when absent.
    pub(crate) auth: String,
    /// The JSON body, `Null` when empty.
    pub(crate) body: Value,
    /// The JSON the fake answered with.
    pub(crate) reply: Value,
}

/// A message the controller posted with a valid bot token.
#[derive(Clone, Debug)]
pub(crate) struct Post {
    /// Channel id.
    pub(crate) channel: String,
    /// Thread root ts.
    pub(crate) thread_ts: String,
    /// Message text.
    pub(crate) text: String,
    /// The ts the fake assigned to the message.
    pub(crate) ts: String,
    /// Block Kit blocks, `Null` when absent.
    pub(crate) blocks: Value,
}

/// A `chat.update` the controller made with a valid bot token.
#[derive(Clone, Debug)]
pub(crate) struct Update {
    /// Channel id.
    pub(crate) channel: String,
    /// Ts of the message edited.
    pub(crate) ts: String,
    /// New message text.
    pub(crate) text: String,
    /// New Block Kit blocks, `Null` when absent.
    pub(crate) blocks: Value,
}

/// A `chat.postEphemeral` the controller made with a valid bot token.
#[derive(Clone, Debug)]
pub(crate) struct Ephemeral {
    /// Channel id.
    pub(crate) channel: String,
    /// The only user who sees the message.
    pub(crate) user: String,
    /// Thread root ts.
    pub(crate) thread_ts: String,
    /// Message text.
    pub(crate) text: String,
}

/// A button press to push to the controller.
pub(crate) struct Click<'a> {
    /// Envelope id the controller must ack.
    pub(crate) envelope_id: &'a str,
    /// Channel of the prompt message.
    pub(crate) channel: &'a str,
    /// User who pressed the button.
    pub(crate) user: &'a str,
    /// Ts of the prompt message.
    pub(crate) message_ts: &'a str,
    /// Thread root ts of the prompt message.
    pub(crate) thread_ts: &'a str,
    /// The button's value.
    pub(crate) value: &'a str,
    /// The button's action id.
    pub(crate) action_id: &'a str,
}

/// A plain message to push to the controller.
pub(crate) struct Reply<'a> {
    /// Event id Slack would deduplicate on.
    pub(crate) event_id: &'a str,
    /// Envelope id the controller must ack.
    pub(crate) envelope_id: &'a str,
    /// Channel the message was posted in.
    pub(crate) channel: &'a str,
    /// Author's user id.
    pub(crate) user: &'a str,
    /// Message text.
    pub(crate) text: &'a str,
    /// Message ts.
    pub(crate) ts: &'a str,
    /// Thread root ts.
    pub(crate) thread_ts: &'a str,
    /// Message subtype, such as `message_changed`.
    pub(crate) subtype: Option<&'a str>,
    /// Bot id when a bot wrote the message.
    pub(crate) bot_id: Option<&'a str>,
}

/// An app mention to push to the controller.
pub(crate) struct Mention<'a> {
    /// Event id Slack would deduplicate on.
    pub(crate) event_id: &'a str,
    /// Envelope id the controller must ack.
    pub(crate) envelope_id: &'a str,
    /// Channel the message was posted in.
    pub(crate) channel: &'a str,
    /// Author's user id.
    pub(crate) user: &'a str,
    /// Message text.
    pub(crate) text: &'a str,
    /// Message ts.
    pub(crate) ts: &'a str,
    /// Thread root ts when the message is a reply.
    pub(crate) thread_ts: Option<&'a str>,
}

/// What the fake has seen and has queued to send.
#[derive(Default)]
struct State {
    /// Every Web API request, in arrival order.
    requests: Vec<Recorded>,
    /// Envelope ids the controller acknowledged over the socket.
    acks: Vec<String>,
    /// The `payload` of each ack that carried one, by envelope id.
    ack_payloads: Vec<(String, Value)>,
    /// Frames waiting for a socket connection to carry them.
    outbox: VecDeque<String>,
    /// Socket connections accepted so far.
    connections: usize,
    /// Messages posted so far, used to hand out unique ts values.
    posted: usize,
    /// HTTP status and body of the next `chat.update` answers, before the fake takes edits.
    update_answers: VecDeque<(u16, Value)>,
    /// HTTP status and body of the next `chat.postMessage` answers without blocks, before the
    /// fake takes posts.
    post_answers: VecDeque<(u16, Value)>,
    /// How many of the next `chat.postMessage` calls with blocks, other than a status message,
    /// Slack refuses, and the error code it refuses them with.
    refused_block_posts: (usize, &'static str),
    /// The `error` code `auth.test` answers with instead of the team, once set.
    auth_test_error: Option<&'static str>,
    /// The profile email `users.info` reports, by Slack user id.
    profile_emails: HashMap<String, String>,
    /// The `error` code `users.info` answers with instead of a profile, once set.
    users_info_error: Option<&'static str>,
    /// The `error` code `reactions.add` and `reactions.remove` answer with, once set.
    reaction_error: Option<&'static str>,
    /// The reactions now on messages, as `(channel, ts, name)`.
    reactions: Vec<(String, String, String)>,
}

/// The fake Slack: a Web API on one port and a Socket Mode endpoint on another.
pub(crate) struct FakeSlack {
    /// Shared with the serving threads.
    state: Arc<Mutex<State>>,
    /// Port of the Web API listener.
    http_port: u16,
}

impl FakeSlack {
    /// Starts both listeners on loopback ports.
    pub(crate) fn start() -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let ws = TcpListener::bind("127.0.0.1:0").unwrap();
        let ws_port = ws.local_addr().unwrap().port();
        let http = TcpListener::bind("127.0.0.1:0").unwrap();
        let http_port = http.local_addr().unwrap().port();
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            for stream in ws.incoming().flatten() {
                serve_socket(stream, &shared);
            }
        });
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            for stream in http.incoming().flatten() {
                let shared = Arc::clone(&shared);
                thread::spawn(move || serve_http(stream, &shared, ws_port));
            }
        });
        Self { state, http_port }
    }

    /// The `api_base` to put in the controller config.
    pub(crate) fn api_base(&self) -> String {
        format!("http://127.0.0.1:{}", self.http_port)
    }

    /// Answers the next `chat.update` calls with `answers`, each an HTTP status and a JSON
    /// body; a 429 asks for one second's wait. Later calls succeed.
    pub(crate) fn answer_updates(&self, answers: &[(u16, Value)]) {
        self.lock().update_answers.extend(answers.iter().cloned());
    }

    /// Answers the next `chat.postMessage` calls that carry no blocks with `answers`, each an
    /// HTTP status and a JSON body. Later calls succeed.
    pub(crate) fn answer_posts(&self, answers: &[(u16, Value)]) {
        self.lock().post_answers.extend(answers.iter().cloned());
    }

    /// Answers every later `auth.test` with `ok: false` and this error code.
    pub(crate) fn refuse_auth_test(&self, code: &'static str) {
        self.lock().auth_test_error = Some(code);
    }
    /// Answers every later `reactions.add` and `reactions.remove` with `ok: false` and this
    /// error code.
    pub(crate) fn refuse_reactions(&self, code: &'static str) {
        self.lock().reaction_error = Some(code);
    }

    /// Refuses the next `n` `chat.postMessage` calls that carry blocks, except a Box's status
    /// message (its text starts with the Box's name in bold), with error `code`, in place of
    /// any count set before.
    pub(crate) fn refuse_block_posts(&self, n: usize, code: &'static str) {
        self.lock().refused_block_posts = (n, code);
    }

    /// Queues an `events_api` envelope carrying `mention`.
    pub(crate) fn send_mention(&self, mention: &Mention<'_>) {
        let mut event = json!({
            "type": "app_mention",
            "user": mention.user,
            "text": mention.text,
            "ts": mention.ts,
            "channel": mention.channel,
        });
        if let Some(thread) = mention.thread_ts {
            event["thread_ts"] = json!(thread);
        }
        self.queue(mention.envelope_id, mention.event_id, &event);
    }

    /// Queues an `events_api` envelope carrying a plain `message` event.
    pub(crate) fn send_reply(&self, reply: &Reply<'_>) {
        let mut event = json!({
            "type": "message",
            "user": reply.user,
            "text": reply.text,
            "ts": reply.ts,
            "channel": reply.channel,
            "thread_ts": reply.thread_ts,
        });
        if let Some(subtype) = reply.subtype {
            event["subtype"] = json!(subtype);
        }
        if let Some(bot_id) = reply.bot_id {
            event["bot_id"] = json!(bot_id);
        }
        self.queue(reply.envelope_id, reply.event_id, &event);
    }

    /// Queues an `interactive` envelope carrying a `block_actions` button press.
    pub(crate) fn send_click(&self, click: &Click<'_>) {
        let envelope = json!({
            "type": "interactive",
            "envelope_id": click.envelope_id,
            "payload": {
                "type": "block_actions",
                "user": {"id": click.user},
                "channel": {"id": click.channel},
                "container": {
                    "type": "message",
                    "message_ts": click.message_ts,
                    "channel_id": click.channel,
                },
                "message": {"ts": click.message_ts, "thread_ts": click.thread_ts},
                "actions": [{"action_id": click.action_id, "value": click.value}],
            },
        });
        self.lock().outbox.push_back(envelope.to_string());
    }

    /// The bodies of the `method` calls, such as `views.open`, made with the bot token.
    pub(crate) fn views(&self, method: &str) -> Vec<Value> {
        let path = format!("/{method}");
        self.requests()
            .into_iter()
            .filter(|r| r.path == path && r.auth == format!("Bearer {BOT_TOKEN}"))
            .map(|r| r.body)
            .collect()
    }

    /// Waits up to `limit` until `method` was called `n` times; returns the bodies.
    pub(crate) fn wait_views(&self, method: &str, n: usize, limit: Duration) -> Vec<Value> {
        for _ in 0..(limit.as_millis() / POLL.as_millis()) {
            let views = self.views(method);
            if views.len() >= n {
                return views;
            }
            thread::sleep(POLL);
        }
        panic!("expected {n} {method} calls, saw {:?}", self.views(method));
    }

    /// Waits for the ack of `envelope_id` and returns its payload.
    pub(crate) fn wait_ack_payload(&self, envelope_id: &str) -> Value {
        let found = || {
            let payloads = self.lock().ack_payloads.clone();
            payloads
                .into_iter()
                .find(|(id, _)| id == envelope_id)
                .map(|(_, payload)| payload)
        };
        Self::wait(found).unwrap_or_else(|| panic!("no ack with a payload for {envelope_id}"))
    }

    /// Messages posted at the top level of a channel with the bot token, in order.
    pub(crate) fn channel_posts(&self) -> Vec<Post> {
        self.posts()
            .into_iter()
            .filter(|p| p.thread_ts.is_empty())
            .collect()
    }

    fn queue(&self, envelope_id: &str, event_id: &str, event: &Value) {
        let envelope = json!({
            "type": "events_api",
            "envelope_id": envelope_id,
            "payload": {
                "event_id": event_id,
                "event": event,
                "authorizations": [{"user_id": "UBOT"}],
            },
        });
        self.lock().outbox.push_back(envelope.to_string());
    }

    /// Every Web API request seen so far.
    pub(crate) fn requests(&self) -> Vec<Recorded> {
        self.lock().requests.clone()
    }

    /// Messages posted with the bot token, in order.
    pub(crate) fn posts(&self) -> Vec<Post> {
        self.requests()
            .iter()
            .filter(|r| r.path == "/chat.postMessage" && r.auth == format!("Bearer {BOT_TOKEN}"))
            .map(|r| {
                let text = |key: &str| r.body[key].as_str().unwrap_or_default().to_owned();
                Post {
                    channel: text("channel"),
                    thread_ts: text("thread_ts"),
                    text: text("text"),
                    ts: r.reply["ts"].as_str().unwrap_or_default().to_owned(),
                    blocks: r.body["blocks"].clone(),
                }
            })
            .collect()
    }

    /// Message edits made with the bot token, in order.
    pub(crate) fn updates(&self) -> Vec<Update> {
        self.requests()
            .iter()
            .filter(|r| r.path == "/chat.update" && r.auth == format!("Bearer {BOT_TOKEN}"))
            .map(|r| {
                let text = |key: &str| r.body[key].as_str().unwrap_or_default().to_owned();
                Update {
                    channel: text("channel"),
                    ts: text("ts"),
                    text: text("text"),
                    blocks: r.body["blocks"].clone(),
                }
            })
            .collect()
    }

    /// Ephemeral messages sent with the bot token, in order.
    pub(crate) fn ephemerals(&self) -> Vec<Ephemeral> {
        self.requests()
            .iter()
            .filter(|r| r.path == "/chat.postEphemeral" && r.auth == format!("Bearer {BOT_TOKEN}"))
            .map(|r| {
                let text = |key: &str| r.body[key].as_str().unwrap_or_default().to_owned();
                Ephemeral {
                    channel: text("channel"),
                    user: text("user"),
                    thread_ts: text("thread_ts"),
                    text: text("text"),
                }
            })
            .collect()
    }

    /// Waits until at least `n` ephemeral messages exist and returns all of them.
    pub(crate) fn wait_ephemerals(&self, n: usize) -> Vec<Ephemeral> {
        Self::wait(|| Some(self.ephemerals()).filter(|e| e.len() >= n))
            .unwrap_or_else(|| panic!("expected {n} ephemerals, saw {:?}", self.ephemerals()))
    }

    /// Waits until at least `n` updates exist and returns all of them.
    pub(crate) fn wait_updates(&self, n: usize) -> Vec<Update> {
        Self::wait(|| Some(self.updates()).filter(|u| u.len() >= n))
            .unwrap_or_else(|| panic!("expected {n} updates, saw {:?}", self.updates()))
    }

    /// Waits until some post or update satisfies `want`; returns the posts and updates then seen.
    pub(crate) fn wait_until(
        &self,
        what: &str,
        want: impl Fn(&[Post], &[Update]) -> bool,
    ) -> (Vec<Post>, Vec<Update>) {
        Self::wait(|| {
            let (posts, updates) = (self.posts(), self.updates());
            want(&posts, &updates).then_some((posts, updates))
        })
        .unwrap_or_else(|| {
            panic!(
                "never saw {what}; posts {:?}, updates {:?}",
                self.posts(),
                self.updates()
            )
        })
    }

    /// Envelope ids acknowledged so far.
    pub(crate) fn acks(&self) -> Vec<String> {
        self.lock().acks.clone()
    }

    /// Waits until at least `n` posts exist and returns all of them.
    pub(crate) fn wait_posts(&self, n: usize) -> Vec<Post> {
        Self::wait(|| Some(self.posts()).filter(|p| p.len() >= n))
            .unwrap_or_else(|| panic!("expected {n} posts, saw {:?}", self.posts()))
    }

    /// Waits until at least `n` acks exist and returns all of them.
    pub(crate) fn wait_acks(&self, n: usize) -> Vec<String> {
        Self::wait(|| Some(self.acks()).filter(|a| a.len() >= n))
            .unwrap_or_else(|| panic!("expected {n} acks, saw {:?}", self.acks()))
    }

    /// Waits until the controller has opened a socket connection.
    pub(crate) fn wait_connected(&self) {
        Self::wait(|| Some(()).filter(|()| self.lock().connections > 0))
            .expect("controller never connected to the socket");
    }

    /// Polls `probe` until it yields a value or the deadline passes.
    fn wait<T>(probe: impl Fn() -> Option<T>) -> Option<T> {
        for _ in 0..(DEADLINE.as_millis() / POLL.as_millis()) {
            if let Some(found) = probe() {
                return Some(found);
            }
            thread::sleep(POLL);
        }
        None
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }
}

/// Serves one Socket Mode connection until it closes.
fn serve_socket(stream: TcpStream, state: &Mutex<State>) {
    let Ok(mut ws) = tungstenite::accept(stream) else {
        return;
    };
    let tick = ws.get_ref().set_read_timeout(Some(POLL));
    if tick.is_err() {
        return;
    }
    state.lock().unwrap().connections += 1;
    let hello = json!({"type": "hello", "num_connections": 1}).to_string();
    if ws.send(Message::text(hello)).is_err() {
        return;
    }
    loop {
        let next = state.lock().unwrap().outbox.pop_front();
        if let Some(frame) = next {
            if ws.send(Message::text(frame.clone())).is_err() {
                state.lock().unwrap().outbox.push_front(frame);
                return;
            }
        }
        match ws.read() {
            Ok(Message::Text(text)) => {
                let ack: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                if let Some(id) = ack["envelope_id"].as_str() {
                    let mut state = state.lock().unwrap();
                    state.acks.push(id.to_owned());
                    if let Some(payload) = ack.get("payload") {
                        state.ack_payloads.push((id.to_owned(), payload.clone()));
                    }
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return,
        }
    }
}

/// Answers one Web API request and closes the connection.
fn serve_http(mut stream: TcpStream, state: &Mutex<State>, ws_port: u16) {
    let Some((path, auth, body)) = read_request(&mut stream) else {
        return;
    };
    let mut guard = state.lock().unwrap();
    let mut status = 200;
    let reply = match path.as_str() {
        "/apps.connections.open" if auth == format!("Bearer {APP_TOKEN}") => {
            json!({"ok": true, "url": format!("ws://127.0.0.1:{ws_port}/")})
        }
        "/auth.test" if auth == format!("Bearer {BOT_TOKEN}") => guard.auth_test_error.map_or(
            json!({"ok": true, "team_id": TEAM}),
            |code| json!({"ok": false, "error": code}),
        ),
        "/chat.postMessage" if auth == format!("Bearer {BOT_TOKEN}") => {
            let (code, reply) = guard.post(&body);
            status = code;
            reply
        }
        "/chat.update" if auth == format!("Bearer {BOT_TOKEN}") => {
            let ok = json!({"ok": true, "channel": body["channel"], "ts": body["ts"]});
            let (code, reply) = guard.update_answers.pop_front().unwrap_or((200, ok));
            status = code;
            reply
        }
        "/reactions.add" | "/reactions.remove" | "/users.info"
            if auth == format!("Bearer {BOT_TOKEN}") =>
        {
            guard.state_reply(&path, &body)
        }
        path if path.starts_with("/respond/") => json!({"ok": true}),
        "/chat.postEphemeral" if auth == format!("Bearer {BOT_TOKEN}") => {
            json!({"ok": true, "message_ts": message_ts(guard.posted)})
        }
        "/views.open" | "/views.update" if auth == format!("Bearer {BOT_TOKEN}") => {
            json!({"ok": true, "view": {"id": "V1"}})
        }
        _ => json!({"ok": false, "error": "invalid_auth"}),
    };
    guard.requests.push(Recorded {
        path,
        auth,
        body,
        reply: reply.clone(),
    });
    drop(guard);
    let text = reply.to_string();
    let response = format!(
        "HTTP/1.1 {status} X\r\nRetry-After: 1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

/// The ts the fake gives the `n`th posted message.
fn message_ts(n: usize) -> String {
    format!("9000000000.{n:06}")
}
