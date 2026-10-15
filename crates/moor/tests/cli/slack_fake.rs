//! An in-process fake of the Slack Web API and Socket Mode, for driving the controller's Slack app.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::Message;

/// App-level token the fake accepts for opening a connection.
pub(crate) const APP_TOKEN: &str = "xapp-test";
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
    /// Frames waiting for a socket connection to carry them.
    outbox: VecDeque<String>,
    /// Socket connections accepted so far.
    connections: usize,
    /// Messages posted so far, used to hand out unique ts values.
    posted: usize,
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
                }
            })
            .collect()
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
                    state.lock().unwrap().acks.push(id.to_owned());
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
    let reply = match path.as_str() {
        "/apps.connections.open" if auth == format!("Bearer {APP_TOKEN}") => {
            json!({"ok": true, "url": format!("ws://127.0.0.1:{ws_port}/")})
        }
        "/chat.postMessage" if auth == format!("Bearer {BOT_TOKEN}") => {
            guard.posted += 1;
            json!({"ok": true, "ts": message_ts(guard.posted)})
        }
        "/chat.update" if auth == format!("Bearer {BOT_TOKEN}") => {
            json!({"ok": true, "channel": body["channel"], "ts": body["ts"]})
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
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

/// The ts the fake gives the `n`th posted message.
fn message_ts(n: usize) -> String {
    format!("9000000000.{n:06}")
}

/// Reads the request line, headers and body; returns path, authorization and JSON body.
fn read_request(stream: &mut TcpStream) -> Option<(String, String, Value)> {
    stream.set_read_timeout(Some(DEADLINE)).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 1024];
    let head_end = loop {
        if let Some(at) = find(&buf, b"\r\n\r\n") {
            break at;
        }
        let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let path = lines.next()?.split(' ').nth(1)?.to_owned();
    let header = |name: &str| {
        lines
            .clone()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_owned())
    };
    let auth = header("authorization").unwrap_or_default();
    let length: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
        body.extend_from_slice(&chunk[..n]);
    }
    let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Some((path, auth, body))
}

/// Index of the first occurrence of `needle` in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
