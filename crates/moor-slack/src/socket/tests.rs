//! Tests for the Socket Mode loop against an in-test WebSocket server on loopback.

#![allow(clippy::indexing_slicing, reason = "tests build JSON fixtures by key")]

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use moor_core::secret::Secret;
use serde_json::json;
use tungstenite::Message;

use super::Click;
use super::{next_delay, parse_mention, run, session, Inbound, Mention, SocketError};
use crate::api::Api;

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
    run(&api, &Secret::new(b"xapp-1".to_vec()), &tx, &|d| {
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
