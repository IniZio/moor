//! Own-bot posts that act as an allow-listed user, for test automation.

#![allow(clippy::indexing_slicing, reason = "tests build JSON fixtures by key")]

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::super::parse::parse_mention;
use super::super::{Mention, Roleplay};

/// A role-play setup for bot `B0OWN` (user `UBOT`) that may act as `UALICE`; `fetched` is what
/// a lookup of the message's metadata returns, and every lookup and log line is recorded.
fn roleplay(fetched: Option<Value>) -> (Roleplay, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let looked = Arc::clone(&seen);
    let logged = Arc::clone(&seen);
    let marked = Arc::clone(&seen);
    let roleplay = Roleplay {
        bot_id: "B0OWN".into(),
        bot_user: "UBOT".into(),
        users: vec!["UALICE".into()],
        metadata: Box::new(move |channel, root, ts| {
            looked
                .lock()
                .expect("lock")
                .push(format!("lookup {channel} {root} {ts}"));
            fetched.clone()
        }),
        log: Arc::new(move |line| logged.lock().expect("lock").push(line.to_owned())),
        mark: Box::new(move |channel, ts| {
            marked
                .lock()
                .expect("lock")
                .push(format!("mark {channel} {ts}"));
        }),
    };
    (roleplay, seen)
}

fn as_user(user: &str) -> Value {
    json!({"event_type":"moor_roleplay","event_payload":{"user":user}})
}

/// A message the bot `B0OWN` posted in `C1`, in thread `1.0` when `thread` is set.
fn own_post(text: &str, thread: bool, metadata: Option<Value>) -> Value {
    let mut event = json!({"type":"message","user":"UBOT","bot_id":"B0OWN","text":text,"ts":"2.0","channel":"C1"});
    if thread {
        event["thread_ts"] = json!("1.0");
    }
    if let Some(metadata) = metadata {
        event["metadata"] = metadata;
    }
    json!({"type":"events_api","envelope_id":"env","payload":{
        "event_id":"Ev1","event":event,"authorizations":[{"user_id":"UBOT"}]}})
}

fn parsed(envelope: &Value, fetched: Option<Value>) -> Option<Mention> {
    let (roleplay, _) = roleplay(fetched);
    parse_mention(envelope, Some(&roleplay))
}

#[test]
fn with_roleplay_off_an_own_post_with_valid_metadata_is_dropped() {
    let envelope = own_post("<@UBOT> start", false, Some(as_user("UALICE")));
    assert!(parse_mention(&envelope, None).is_none());
}

#[test]
fn a_top_level_own_post_naming_the_bot_starts_a_thread_as_the_user() {
    let envelope = own_post("<@UBOT> start", false, Some(as_user("UALICE")));
    let mention = parsed(&envelope, None).expect("mention");
    assert_eq!(mention.user, "UALICE");
    assert_eq!(mention.channel, "C1");
    assert_eq!(mention.ts, "2.0");
    assert_eq!(mention.thread_ts, None);
    assert!(!mention.reply);
}

#[test]
fn an_own_post_in_a_thread_is_a_reply_from_the_user() {
    let envelope = own_post("steer", true, Some(as_user("UALICE")));
    let mention = parsed(&envelope, None).expect("reply");
    assert_eq!(mention.user, "UALICE");
    assert_eq!(mention.thread_ts.as_deref(), Some("1.0"));
    assert!(mention.reply);
}

#[test]
fn an_own_post_naming_the_bot_in_a_thread_is_a_mention_from_the_user() {
    let envelope = own_post("<@UBOT> again", true, Some(as_user("UALICE")));
    let mention = parsed(&envelope, None).expect("mention");
    assert_eq!(mention.user, "UALICE");
    assert!(!mention.reply);
}

#[test]
fn metadata_missing_from_the_event_is_looked_up_by_channel_thread_and_ts() {
    let (roleplay, seen) = roleplay(Some(as_user("UALICE")));
    let envelope = own_post("steer", true, None);
    let mention = parse_mention(&envelope, Some(&roleplay)).expect("reply");
    assert_eq!(mention.user, "UALICE");
    let seen = seen.lock().expect("lock").clone();
    assert_eq!(seen.first().map(String::as_str), Some("lookup C1 1.0 2.0"));
}

#[test]
fn each_roleplayed_message_logs_its_channel_ts_and_user() {
    let (roleplay, seen) = roleplay(None);
    let envelope = own_post("steer", true, Some(as_user("UALICE")));
    assert!(parse_mention(&envelope, Some(&roleplay)).is_some());
    let seen: Vec<String> = seen.lock().expect("lock").clone();
    let logged: Vec<&String> = seen.iter().filter(|l| !l.starts_with("mark ")).collect();
    assert_eq!(logged.len(), 1, "{seen:?}");
    let line = logged[0];
    for part in ["C1", "2.0", "UALICE"] {
        assert!(line.contains(part), "{line}");
    }
}

#[test]
fn own_posts_without_valid_roleplay_metadata_are_dropped() {
    let other_type = json!({"event_type":"ticket_created","event_payload":{"user":"UALICE"}});
    let mut other_bot = own_post("steer", true, Some(as_user("UALICE")));
    other_bot["payload"]["event"]["bot_id"] = json!("B0OTHER");
    let cases = [
        (
            "unlisted user",
            own_post("steer", true, Some(as_user("UMALLORY"))),
        ),
        ("other bot", other_bot),
        ("no metadata", own_post("steer", true, None)),
        (
            "other event type",
            own_post("steer", true, Some(other_type)),
        ),
        (
            "user named in text",
            own_post("<@UBOT> as <@UALICE>", false, None),
        ),
        (
            "top level without the bot",
            own_post("hi", false, Some(as_user("UALICE"))),
        ),
    ];
    for (name, envelope) in cases {
        assert!(parsed(&envelope, None).is_none(), "{name}: {envelope}");
    }
}

#[test]
fn a_user_post_is_unchanged_with_roleplay_on() {
    let event = json!({"type":"message","user":"U1","text":"steer","ts":"2.0","thread_ts":"1.0","channel":"C1"});
    let envelope =
        json!({"type":"events_api","envelope_id":"env","payload":{"event_id":"Ev1","event":event}});
    let mention = parsed(&envelope, None).expect("reply");
    assert_eq!(mention.user, "U1");
}

#[test]
fn each_roleplayed_message_is_marked_and_a_dropped_one_is_not() {
    let (roleplay, seen) = roleplay(None);
    let taken = own_post("steer", true, Some(as_user("UALICE")));
    assert!(parse_mention(&taken, Some(&roleplay)).is_some());
    let dropped = own_post("steer", true, Some(as_user("UMALLORY")));
    assert!(parse_mention(&dropped, Some(&roleplay)).is_none());
    let marks: Vec<String> = seen
        .lock()
        .expect("lock")
        .iter()
        .filter(|l| l.starts_with("mark "))
        .cloned()
        .collect();
    assert_eq!(marks, ["mark C1 2.0"]);
}
