//! Tests for reading Claude's transcript into timeline lines.

use serde_json::{json, Value};

use super::*;
use crate::progress::timeline_with;
use crate::progress::Class;
use crate::view::EventView;

const CWD: &str = "/work/box-1/web";

fn at(s: u32) -> String {
    format!("2026-11-19T10:00:{s:02}.000Z")
}

fn assistant(s: u32, uuid: &str, content: &Value) -> String {
    json!({"type": "assistant", "uuid": uuid, "timestamp": at(s), "cwd": CWD,
           "message": {"role": "assistant", "content": content}})
    .to_string()
}

fn use_(s: u32, id: &str, name: &str, input: &Value) -> String {
    let block = json!([{"type": "tool_use", "id": id, "name": name, "input": input}]);
    assistant(s, &format!("u-{id}"), &block)
}

fn result(s: u32, id: &str, text: &str, error: bool, detail: &Value) -> String {
    json!({"type": "user", "timestamp": at(s), "toolUseResult": detail,
           "message": {"role": "user", "content": [
               {"type": "tool_result", "tool_use_id": id, "content": text, "is_error": error}]}})
    .to_string()
}

fn session() -> Vec<String> {
    let file = format!("{CWD}/src/login.tsx");
    let patch = json!({"structuredPatch": [
        {"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1,
         "lines": ["-const label = 'Lgoin';", "+const label = 'Login';"]}]});
    let thought = json!([{"type": "thinking", "thinking": "secret plan"}, {"type": "text", "text": "Looking."}]);
    let none = json!({});
    vec![
        json!({"type": "user", "timestamp": at(0), "message": {"content": "Fix the login typo"}})
            .to_string(),
        assistant(1, "a1", &thought),
        use_(2, "t1", "Read", &json!({"file_path": file})),
        result(3, "t1", "1\tconst label = 'Lgoin';", false, &none),
        use_(4, "t2", "Grep", &json!({"pattern": "Lgoin", "path": format!("{CWD}/src")})),
        result(5, "t2", "src/login.tsx", false, &none),
        use_(6, "t3", "Edit", &json!({"file_path": file, "old_string": "Lgoin", "new_string": "Login"})),
        result(7, "t3", "The file has been updated.", false, &patch),
        use_(8, "t4", "Bash", &json!({"command": "npm test"})),
        result(9, "t4", "Exit code 1\nFAIL login.test.tsx", true, &none),
        use_(10, "t5", "Bash", &json!({"command": "git push --force"})),
        result(11, "t5", "The user doesn't want to proceed with this tool use.", true, &none),
        json!({"type": "assistant", "isSidechain": true, "uuid": "s1", "timestamp": at(12),
               "message": {"content": [{"type": "tool_use", "id": "x1", "name": "Read", "input": {}}]}})
        .to_string(),
    ]
}

fn read(records: &[String]) -> Vec<Line> {
    extract(records.iter().map(String::as_str))
}

fn tools(lines: &[Line]) -> Vec<&Tool> {
    lines
        .iter()
        .filter_map(|l| match &l.what {
            What::Tool(t) => Some(t),
            What::Said { .. } => None,
        })
        .collect()
}

#[test]
fn each_call_reads_as_one_line_and_subagents_are_left_out() {
    let lines = read(&session());
    let titles: Vec<&str> = tools(&lines).iter().map(|t| t.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Read src/login.tsx",
            "Searched for Lgoin",
            "Edited src/login.tsx",
            "Ran npm test (exit 1)",
            "Ran git push --force (refused)",
        ]
    );
    let groups: Vec<Option<Group>> = tools(&lines).iter().map(|t| t.group).collect();
    assert_eq!(
        groups,
        [Some(Group::Read), Some(Group::Search), None, None, None]
    );
    let failed: Vec<bool> = tools(&lines).iter().map(|t| t.failed).collect();
    assert_eq!(failed, [false, false, false, true, true]);
}

#[test]
fn the_agent_s_words_are_kept_and_its_thinking_is_not() {
    let lines = read(&session());
    let first = lines.first().map(|l| &l.what);
    assert_eq!(
        first,
        Some(&What::Said {
            text: "Looking.".into()
        })
    );
    assert!(!serde_json::to_string(&lines)
        .unwrap()
        .contains("secret plan"));
}

#[test]
fn an_edit_shows_its_diff_and_a_command_its_input_and_output() {
    let lines = read(&session());
    let [_, _, edit, test, _]: [&Tool; 5] = tools(&lines).try_into().unwrap();
    assert_eq!(
        edit.diff.as_ref().map(|d| d.text.as_str()),
        Some("@@ -1,1 +1,1 @@\n-const label = 'Lgoin';\n+const label = 'Login';\n")
    );
    assert!(
        edit.output.is_none(),
        "an edit that worked shows its diff, not Claude's note"
    );
    assert_eq!(
        test.input.as_ref().map(|b| b.text.as_str()),
        Some("npm test")
    );
    let out = test
        .output
        .as_ref()
        .map(|b| b.text.as_str())
        .unwrap_or_default();
    assert!(out.contains("FAIL login.test.tsx"));
}

#[test]
fn a_record_repeated_in_a_second_session_file_is_kept_once() {
    let records = session();
    let twice: Vec<String> = records.iter().chain(records.iter()).cloned().collect();
    assert_eq!(read(&twice), read(&records));
}

#[test]
fn a_long_output_keeps_its_start_and_its_end() {
    let long = format!("start\n{}\nthe error", "x".repeat(20_000));
    let records = [
        use_(1, "t", "Bash", &json!({"command": "make"})),
        result(2, "t", &long, true, &json!({})),
    ];
    let lines = read(&records);
    let [tool]: [&Tool; 1] = tools(&lines).try_into().unwrap();
    let out = tool.output.clone().unwrap();
    assert!(out.cut);
    assert!(out.text.starts_with("start"));
    assert!(out.text.ends_with("the error"));
    assert!(out.text.chars().count() < 6100);
}

#[test]
fn the_oldest_lines_go_past_the_cap_and_lose_their_bodies_past_the_budget() {
    let big = "y".repeat(5000);
    let records: Vec<String> = (0..700u32)
        .flat_map(|n| {
            let id = format!("t{n}");
            let s = n % 60;
            let cmd = json!({"command": format!("echo {n}")});
            [
                use_(s, &id, "Bash", &cmd),
                result(s, &id, &big, false, &json!({})),
            ]
        })
        .collect();
    let lines = read(&records);
    assert_eq!(lines.len(), MAX_LINES);
    let t = tools(&lines);
    let kept: usize = t
        .iter()
        .flat_map(|t| [&t.input, &t.output])
        .filter_map(|b| b.as_ref().map(|b| b.text.len()))
        .sum();
    assert!(kept <= BUDGET);
    assert!(t.last().unwrap().output.is_some());
    assert!(t.first().unwrap().output.is_none());
}

#[test]
fn every_scrub_rule_compiles() {
    assert_eq!(scrub::compiled(), 5);
}

#[test]
fn credentials_are_scrubbed_and_code_is_not() {
    let cases = [
        ("GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123", "ghp_"),
        ("key sk-ant-api03-abcdefghijklmnop", "sk-ant-api03"),
        (
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz",
            "abcdefghijklmnopqrstuvwxyz",
        ),
        (
            "git clone https://user:hunter2hunter2@github.com/a/b",
            "hunter2",
        ),
        ("export API_KEY=abcd1234efgh5678", "abcd1234efgh5678"),
        ("{\"password\": \"pw\"}", "\"pw\""),
        ("aws AKIAABCDEFGHIJKLMNOP", "AKIAABCDEFGHIJKLMNOP"),
        (
            "-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----",
            "MIIE",
        ),
        ("xoxb-1234567890-abcdefghij", "xoxb-"),
    ];
    for (text, secret) in cases {
        let out = scrub(text);
        assert!(!out.contains(secret), "{text} -> {out}");
        assert!(out.contains("[redacted"), "{text} -> {out}");
    }
    for code in [
        "let token = read_token();",
        "api_key: Option<String>,",
        "the password is wrong",
    ] {
        assert_eq!(scrub(code), code);
    }
}

#[test]
fn a_tool_input_and_output_are_scrubbed() {
    let curl = json!({"command": "curl -H 'Authorization: Bearer abcdefghijklmnopqrstu' x"});
    let records = [
        use_(1, "t", "Bash", &curl),
        result(
            2,
            "t",
            "token ghp_abcdefghijklmnopqrstuvwxyz0123",
            false,
            &json!({}),
        ),
    ];
    let shown = serde_json::to_string(&read(&records)).unwrap();
    assert!(!shown.contains("abcdefghijklmnopqrstu"));
    assert!(!shown.contains("ghp_abc"));
}

fn ev(id: i64, kind: &str, payload: Value) -> EventView {
    EventView {
        id,
        kind: kind.to_owned(),
        payload,
        ts: "t".to_owned(),
    }
}

fn ms(s: u32) -> i64 {
    at(s).parse::<jiff::Timestamp>().unwrap().as_millisecond()
}

#[test]
fn lines_sit_among_events_by_time_and_replace_the_steps_they_cover() {
    let lines = read(&session());
    let events = [
        (
            ms(0),
            ev(1, "steered", json!({"message": "Fix the login typo"})),
        ),
        (ms(5), ev(2, "agent_doing", json!({"text": "Grep Lgoin"}))),
        (ms(12), ev(3, "agent_said", json!({"text": "Done."}))),
        (ms(13), ev(4, "turn_done", json!({"outcome": "idle"}))),
        (ms(30), ev(5, "agent_doing", json!({"text": "Bash ls"}))),
    ];
    let saved = Transcript {
        lines: &lines,
        saved_ms: ms(20),
    };
    let t = timeline_with(&events, saved);
    let shown: Vec<(&str, Class)> = t.iter().map(|e| (e.kind.as_str(), e.class)).collect();
    assert_eq!(
        shown,
        [
            ("steered", Class::You),
            ("said", Class::Message),
            ("tool", Class::Step),
            ("tool", Class::Step),
            ("tool", Class::Step),
            ("tool", Class::Error),
            ("tool", Class::Error),
            ("turn_done", Class::Turn),
            ("agent_doing", Class::Step),
        ]
    );
    let keys: Vec<Option<&str>> = t.iter().map(|e| e.key.as_deref()).collect();
    assert_eq!(keys.get(2), Some(&Some("t1")));
}

#[test]
fn without_lines_the_one_line_steps_stay() {
    let events = [(ms(5), ev(2, "agent_doing", json!({"text": "Grep Lgoin"})))];
    let none = Transcript {
        lines: &[],
        saved_ms: ms(20),
    };
    assert_eq!(timeline_with(&events, none).len(), 1);
}

#[test]
fn a_message_older_than_the_first_line_kept_stays() {
    let lines = read(&session());
    let events = [
        (
            ms(0) - 60_000,
            ev(1, "agent_said", json!({"text": "An early turn."})),
        ),
        (ms(5), ev(2, "agent_doing", json!({"text": "Grep Lgoin"}))),
    ];
    let saved = Transcript {
        lines: &lines,
        saved_ms: ms(20),
    };
    let kinds: Vec<String> = timeline_with(&events, saved)
        .into_iter()
        .filter(|e| e.kind.starts_with("agent_"))
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds, ["agent_said"]);
}

#[test]
fn the_stored_lines_stay_under_their_cap_at_their_worst() {
    let said = "z".repeat(SAID_MAX * 2);
    let pattern = "p".repeat(10_000);
    let records: Vec<String> = (0..800u32)
        .flat_map(|n| {
            let id = format!("t{n}");
            let s = n % 60;
            let text = json!([{"type": "text", "text": said}]);
            let grep = json!({"pattern": pattern});
            [
                assistant(s, &format!("a{n}"), &text),
                use_(s, &id, "Grep", &grep),
                result(s, &id, &said, true, &json!({})),
            ]
        })
        .collect();
    let stored = serde_json::to_vec(&read(&records)).unwrap();
    assert!((stored.len() as u64) < STORED_MAX, "{} bytes", stored.len());
}
