//! Tests for the Socket Mode loop against an in-test WebSocket server on loopback.

#![allow(clippy::indexing_slicing, reason = "tests build JSON fixtures by key")]

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

use moor_core::secret::Secret;
use serde_json::json;
use tungstenite::Message;

use super::parse::parse_mention;
use super::{
    next_delay, run, session, session_tracked, Inbound, Intake, Kept, Mention, NotKept, SocketError,
};

mod proxy;
use super::{AfterAck, Click, Slash, TokenSubmission};
use crate::account::Account;
use crate::api::Api;
use crate::login::Form;

impl Intake for mpsc::Sender<Inbound> {
    fn take(&self, item: Inbound) -> Result<Kept, NotKept> {
        self.send(item)
            .map(|()| Kept::default())
            .map_err(|_| NotKept::Gone)
    }
}

/// Serves one WebSocket client with `script`, returning every text frame it sent back.
fn fake_slack<F>(script: F) -> (String, JoinHandle<Vec<String>>)
where
    F: FnOnce(&mut tungstenite::WebSocket<std::net::TcpStream>) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("ws://{}/link", listener.local_addr().expect("addr"));
    let handle = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().expect("accept");
        tcp.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut ws = tungstenite::accept(tcp).expect("handshake");
        script(&mut ws);
        let mut acks = Vec::new();
        while let Ok(message) = ws.read() {
            match message {
                Message::Text(text) => acks.push(text.to_string()),
                Message::Close(_) => break,
                _ => {}
            }
        }
        acks
    });
    (url, handle)
}

/// Sends a JSON value as a text frame.
fn send(ws: &mut tungstenite::WebSocket<std::net::TcpStream>, value: &serde_json::Value) {
    ws.send(Message::text(value.to_string())).expect("send");
}

/// An `events_api` envelope carrying `event`.
fn events_api(envelope_id: &str, event_id: &str, event: &serde_json::Value) -> serde_json::Value {
    json!({"type":"events_api","envelope_id":envelope_id,"payload":{"event_id":event_id,"event":event}})
}

#[test]
fn acks_every_envelope_and_forwards_only_the_mention() {
    let (url, server) = fake_slack(|ws| {
        send(ws, &json!({"type":"hello","num_connections":1}));
        send(
            ws,
            &events_api(
                "env-1",
                "Ev1",
                &json!({"type":"app_mention","user":"U1","text":"<@UBOT> start x","ts":"1730000000.1","channel":"C1","thread_ts":"1730000000.0"}),
            ),
        );
        send(
            ws,
            &events_api(
                "env-2",
                "Ev2",
                &json!({"type":"message","user":"U1","text":"hi","ts":"1.2","channel":"C1"}),
            ),
        );
        send(
            ws,
            &json!({"type":"disconnect","reason":"refresh_requested"}),
        );
    });
    let (tx, rx) = mpsc::channel();
    session(&url, &tx).expect("session ends cleanly");
    let acks = server.join().expect("server");
    assert_eq!(
        acks,
        vec![r#"{"envelope_id":"env-1"}"#, r#"{"envelope_id":"env-2"}"#]
    );
    let mentions: Vec<Inbound> = rx.try_iter().collect();
    assert_eq!(
        mentions,
        vec![Inbound::Mention(Mention {
            event_id: "Ev1".into(),
            channel: "C1".into(),
            user: "U1".into(),
            text: "<@UBOT> start x".into(),
            ts: "1730000000.1".into(),
            thread_ts: Some("1730000000.0".into()),
            reply: false,
        })]
    );
}

#[test]
fn malformed_envelopes_are_ignored_and_ids_still_acked() {
    let (url, server) = fake_slack(|ws| {
        ws.send(Message::text("not json")).expect("send");
        send(
            ws,
            &json!({"type":"events_api","envelope_id":"env-9","payload":{}}),
        );
        send(
            ws,
            &events_api("env-10", "Ev3", &json!({"type":"app_mention","user":"U1"})),
        );
        ws.close(None).expect("close");
    });
    let (tx, rx) = mpsc::channel();
    session(&url, &tx).expect("session ends cleanly");
    let acks = server.join().expect("server");
    assert_eq!(
        acks,
        vec![r#"{"envelope_id":"env-9"}"#, r#"{"envelope_id":"env-10"}"#]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn answers_pings_while_idle() {
    let (url, server) = fake_slack(|ws| {
        send(ws, &json!({"type":"hello"}));
        ws.send(Message::Ping(vec![7].into())).expect("ping");
        let pong = loop {
            if let Message::Pong(data) = ws.read().expect("read") {
                break data;
            }
        };
        assert_eq!(pong.as_ref(), &[7]);
        send(ws, &json!({"type":"disconnect"}));
    });
    let (tx, _rx) = mpsc::channel();
    session(&url, &tx).expect("session ends cleanly");
    server.join().expect("server");
}

#[test]
fn a_dropped_receiver_ends_the_session() {
    let (url, server) = fake_slack(|ws| {
        let event = json!({"type":"app_mention","user":"U1","text":"t","ts":"1.1","channel":"C1"});
        send(ws, &events_api("env-1", "Ev1", &event));
    });
    let (tx, rx) = mpsc::channel();
    drop(rx);
    assert!(matches!(session(&url, &tx), Err(SocketError::ReceiverGone)));
    server.join().expect("server");
}

#[test]
fn rejects_urls_that_are_not_websocket() {
    let (tx, _rx) = mpsc::channel();
    assert!(matches!(
        session("http://127.0.0.1:1/x", &tx),
        Err(SocketError::Url(_))
    ));
}

#[test]
fn errors_do_not_echo_the_url() {
    let (tx, _rx) = mpsc::channel();
    let err = session("ws://127.0.0.1:1/link?ticket=SECRET", &tx).expect_err("refused");
    assert!(!format!("{err} {err:?}").contains("SECRET"));
}

#[test]
fn backoff_doubles_to_the_cap() {
    let mut delay = Duration::from_secs(1);
    let mut seen = Vec::new();
    for _ in 0..8 {
        seen.push(delay.as_secs());
        delay = next_delay(delay);
    }
    assert_eq!(seen, vec![1, 2, 4, 8, 16, 32, 60, 60]);
}

/// Answers one HTTP request with an `apps.connections.open` reply pointing at `ws_url`.
fn open_endpoint(ws_url: &str) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let body = json!({"ok":true,"url":ws_url}).to_string();
    let handle = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().expect("accept");
        conn.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut buf = [0_u8; 4096];
        let _ = conn.read(&mut buf).expect("read");
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        conn.write_all(reply.as_bytes()).expect("write");
    });
    (base, handle)
}

#[test]
fn run_returns_once_the_receiver_is_gone() {
    let (ws_url, server) = fake_slack(|ws| {
        send(ws, &json!({"type":"hello"}));
        let event = json!({"type":"app_mention","user":"U1","text":"t","ts":"1.1","channel":"C1"});
        send(ws, &events_api("env-1", "Ev1", &event));
    });
    let (base, http) = open_endpoint(&ws_url);
    let api = Api::new(&base).expect("api");
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let slept = RefCell::new(Vec::new());
    let stop = AtomicBool::new(false);
    run(&api, &Secret::new(b"xapp-1".to_vec()), &tx, &stop, &|d| {
        slept.borrow_mut().push(d);
    });
    http.join().expect("http");
    server.join().expect("server");
    assert!(slept.borrow().is_empty());
}

/// An `events_api` envelope whose payload names the bot user `UBOT`.
fn envelope_for(event: &serde_json::Value) -> serde_json::Value {
    let mut envelope = events_api("env", "Ev9", event);
    envelope["payload"]["authorizations"] = json!([{"user_id":"UBOT"}]);
    envelope
}

#[test]
fn thread_reply_is_parsed_as_reply() {
    let event = json!({"type":"message","user":"U1","text":"steer","ts":"2.0","thread_ts":"1.0","channel":"C1"});
    let mention = parse_mention(&envelope_for(&event)).expect("reply");
    assert!(mention.reply);
    assert_eq!(mention.thread_ts.as_deref(), Some("1.0"));
    assert_eq!(mention.text, "steer");
}

#[test]
fn app_mention_is_not_a_reply() {
    let event = json!({"type":"app_mention","user":"U1","text":"<@UBOT> hi","ts":"2.0","thread_ts":"1.0","channel":"C1"});
    assert!(!parse_mention(&envelope_for(&event)).expect("mention").reply);
}

#[test]
fn ignores_messages_that_are_not_user_thread_replies() {
    let base = json!({"type":"message","user":"U1","text":"hi","ts":"2.0","thread_ts":"1.0","channel":"C1"});
    let with = |key: &str, value: serde_json::Value| {
        let mut event = base.clone();
        event[key] = value;
        event
    };
    for event in [
        with("thread_ts", json!("2.0")),
        with("subtype", json!("message_changed")),
        with("subtype", json!("channel_join")),
        with("bot_id", json!("B1")),
        with("text", json!("hey <@UBOT> look")),
    ] {
        assert!(parse_mention(&envelope_for(&event)).is_none(), "{event}");
    }
    let mut top_level = base.clone();
    top_level
        .as_object_mut()
        .expect("object")
        .remove("thread_ts");
    assert!(parse_mention(&envelope_for(&top_level)).is_none());
}

#[test]
fn bot_mention_filter_is_skipped_without_authorizations() {
    let event = json!({"type":"message","user":"U1","text":"<@UBOT> hi","ts":"2.0","thread_ts":"1.0","channel":"C1"});
    assert!(parse_mention(&events_api("env", "Ev9", &event)).is_some());
}

/// An `interactive` envelope with one action.
fn interactive(payload_type: &str, action_id: &str, value: &str) -> serde_json::Value {
    json!({"type":"interactive","envelope_id":"env-c","payload":{"type":payload_type,"user":{"id":"U1"},"channel":{"id":"C1"},"container":{"type":"message","message_ts":"3.0","channel_id":"C1"},"message":{"ts":"3.0","thread_ts":"1.0"},"actions":[{"action_id":action_id,"value":value}]}})
}

/// Runs a session that receives `envelope`, returning the acks and what was forwarded.
fn forwarded(envelope: serde_json::Value) -> (Vec<String>, Vec<Inbound>) {
    let (url, server) = fake_slack(move |ws| {
        send(ws, &envelope);
        ws.close(None).expect("close");
    });
    let (tx, rx) = mpsc::channel();
    session(&url, &tx).expect("session ends cleanly");
    (server.join().expect("server"), rx.try_iter().collect())
}

#[test]
fn block_actions_is_forwarded_as_click() {
    let (acks, got) = forwarded(interactive("block_actions", "answer_2", "2"));
    assert_eq!(acks, vec![r#"{"envelope_id":"env-c"}"#]);
    assert_eq!(
        got,
        vec![Inbound::Click(Click {
            envelope_id: "env-c".into(),
            channel: "C1".into(),
            user: "U1".into(),
            message_ts: "3.0".into(),
            thread_ts: Some("1.0".into()),
            choice: 2,
        })]
    );
}

#[test]
fn malformed_clicks_are_acked_but_not_forwarded() {
    for envelope in [
        interactive("view_submission", "answer_1", "1"),
        interactive("block_actions", "other_1", "1"),
        interactive("block_actions", "answer_x", "x"),
        interactive("block_actions", "answer_0", "0"),
    ] {
        let (acks, got) = forwarded(envelope);
        assert_eq!(acks, vec![r#"{"envelope_id":"env-c"}"#]);
        assert!(got.is_empty());
    }
}

/// An intake that fails to keep every item.
struct Failing;

impl Intake for Failing {
    fn take(&self, _item: Inbound) -> Result<Kept, NotKept> {
        Err(NotKept::Failed)
    }
}

#[test]
fn an_item_the_intake_did_not_keep_gets_no_ack() {
    let mention = json!({"type":"app_mention","user":"U1","text":"t","ts":"1.1","channel":"C1"});
    let (url, server) = fake_slack(move |ws| {
        send(ws, &events_api("env-1", "Ev1", &mention));
        send(
            ws,
            &json!({"type":"events_api","envelope_id":"env-2","payload":{}}),
        );
        ws.close(None).expect("close");
    });
    session(&url, &Failing).expect("session ends cleanly");
    let acks = server.join().expect("server");
    assert_eq!(acks, vec![r#"{"envelope_id":"env-2"}"#]);
}

/// An intake that keeps each item only after a pause, then sets its flag.
struct Slow(Arc<AtomicBool>);

impl Intake for Slow {
    fn take(&self, _item: Inbound) -> Result<Kept, NotKept> {
        std::thread::sleep(Duration::from_millis(300));
        self.0.store(true, Ordering::SeqCst);
        Ok(Kept::default())
    }
}

#[test]
fn the_ack_is_sent_after_the_intake_keeps_the_item() {
    let kept = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&kept);
    let mention = json!({"type":"app_mention","user":"U1","text":"t","ts":"1.1","channel":"C1"});
    let (url, server) = fake_slack(move |ws| {
        send(ws, &events_api("env-1", "Ev1", &mention));
        assert!(ws.read().expect("ack").is_text());
        assert!(
            seen.load(Ordering::SeqCst),
            "the ack came before the item was kept"
        );
        ws.close(None).expect("close");
    });
    session(&url, &Slow(kept)).expect("session ends cleanly");
    server.join().expect("server");
}

/// An intake that keeps each item in a list and answers its ack with fixed text.
struct Answering(RefCell<Vec<Inbound>>);

impl Intake for Answering {
    fn take(&self, item: Inbound) -> Result<Kept, NotKept> {
        self.0.borrow_mut().push(item);
        Ok(Kept {
            ack: Some(json!({"text": "Recorded."})),
            then: None,
        })
    }
}

#[test]
fn a_slash_command_is_forwarded_and_its_ack_carries_the_intake_payload() {
    let command = json!({"type":"slash_commands","envelope_id":"env-s","payload":{"command":"/moor","text":"repo add acme/api","user_id":"U1","channel_id":"C1","trigger_id":"t1"}});
    let (url, server) = fake_slack(move |ws| {
        send(ws, &command);
        ws.close(None).expect("close");
    });
    let intake = Answering(RefCell::default());
    session(&url, &intake).expect("session ends cleanly");
    let acks = server.join().expect("server");
    assert_eq!(
        acks,
        vec![r#"{"envelope_id":"env-s","payload":{"text":"Recorded."}}"#]
    );
    let want = Inbound::Slash(Slash {
        envelope_id: "env-s".into(),
        channel: "C1".into(),
        user: "U1".into(),
        text: "repo add acme/api".into(),
        trigger_id: Some("t1".into()),
        response_url: None,
    });
    assert_eq!(intake.0.into_inner(), vec![want]);
}

/// A `view_submission` envelope of the modal `callback` with `token` in its input.
fn submission(callback: &str, token: &str) -> serde_json::Value {
    json!({"type":"interactive","envelope_id":"env-v","payload":{"type":"view_submission","user":{"id":"U1"},"view":{"id":"V1","callback_id":callback,"private_metadata":"claude","state":{"values":{"token":{"token":{"type":"plain_text_input","value":token}}}}}}})
}

#[test]
fn a_login_submission_is_forwarded_with_its_token_trimmed() {
    let (acks, got) = forwarded(submission("moor_login", "  tok-123\n"));
    assert_eq!(acks, vec![r#"{"envelope_id":"env-v"}"#]);
    let want = Inbound::Submission(TokenSubmission {
        envelope_id: "env-v".into(),
        user: "U1".into(),
        view_id: "V1".into(),
        account: "claude".into(),
        tokens: vec![("token".into(), Secret::new(b"tok-123".to_vec()))],
    });
    assert_eq!(got, vec![want]);
    let (_, got) = forwarded(submission("other_modal", "tok-123"));
    assert!(got.is_empty());
}

/// An intake that asks for the login modal after each ack and records each request.
struct Opening(Arc<AtomicBool>, RefCell<Vec<AfterAck>>);

impl Intake for Opening {
    fn take(&self, _item: Inbound) -> Result<Kept, NotKept> {
        let then = AfterAck::OpenLogin {
            trigger_id: "t1".into(),
            form: Form::One(Account::Claude),
        };
        Ok(Kept {
            ack: None,
            then: Some(then),
        })
    }

    /// Waits for the server to read the ack; an ack still unsent would never arrive.
    fn after_ack(&self, then: AfterAck) {
        let acked = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(20));
            self.0.load(Ordering::SeqCst)
        });
        assert!(acked, "the modal opened before the ack was sent");
        self.1.borrow_mut().push(then);
    }
}

#[test]
fn the_login_modal_opens_after_the_ack() {
    let acked = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&acked);
    let command = json!({"type":"slash_commands","envelope_id":"env-s","payload":{"text":"login","user_id":"U1","channel_id":"C1","trigger_id":"t1"}});
    let (url, server) = fake_slack(move |ws| {
        send(ws, &command);
        assert!(ws.read().expect("ack").is_text());
        seen.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        ws.close(None).expect("close");
    });
    let intake = Opening(acked, RefCell::default());
    session(&url, &intake).expect("session ends cleanly");
    server.join().expect("server");
    let want = AfterAck::OpenLogin {
        trigger_id: "t1".into(),
        form: Form::One(Account::Claude),
    };
    assert_eq!(intake.1.into_inner(), vec![want]);
}

/// An intake that keeps every item and counts them, as SIGTERM arrives while it works.
struct Stopping {
    /// Set when the stop signal arrives.
    stop: Arc<AtomicBool>,
    /// Items kept.
    kept: RefCell<u32>,
}

impl Intake for Stopping {
    fn take(&self, _item: Inbound) -> Result<Kept, NotKept> {
        *self.kept.borrow_mut() += 1;
        self.stop.store(true, Ordering::SeqCst);
        Ok(Kept::default())
    }
}

#[test]
fn a_stop_during_the_intake_keeps_the_event_but_leaves_it_unacked() {
    let mention = json!({"type":"app_mention","user":"U1","text":"t","ts":"1.1","channel":"C1"});
    let (url, server) = fake_slack(move |ws| {
        send(ws, &json!({"type":"hello"}));
        send(ws, &events_api("env-1", "Ev1", &mention));
        send(ws, &events_api("env-2", "Ev2", &mention));
    });
    let stop = Arc::new(AtomicBool::new(false));
    let intake = Stopping {
        stop: Arc::clone(&stop),
        kept: RefCell::new(0),
    };
    session_tracked(&url, &intake, &mut false, &stop).expect("session ends cleanly");
    assert_eq!(*intake.kept.borrow(), 1, "the event in flight was kept");
    assert_eq!(server.join().expect("server"), Vec::<String>::new());
}

#[test]
fn a_stop_that_came_first_reads_nothing_more() {
    let mention = json!({"type":"app_mention","user":"U1","text":"t","ts":"1.1","channel":"C1"});
    let (url, server) = fake_slack(move |ws| send(ws, &events_api("env-1", "Ev1", &mention)));
    let stop = Arc::new(AtomicBool::new(true));
    let intake = Stopping {
        stop: Arc::clone(&stop),
        kept: RefCell::new(0),
    };
    session_tracked(&url, &intake, &mut false, &stop).expect("session ends cleanly");
    assert_eq!(*intake.kept.borrow(), 0);
    assert_eq!(server.join().expect("server"), Vec::<String>::new());
}
