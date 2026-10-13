use moor_core::activity::{HookKind, HookRecord};
use moor_core::seam::agent_profile::{AgentProfile, FirstRunPrompt};
use moor_core::seam::clock::Ts;

use super::Claude;

fn parse(raw: &str) -> Option<HookRecord> {
    Claude.parse_hook(raw.as_bytes())
}

fn kind(raw: &str) -> Option<HookKind> {
    parse(raw).map(|r| r.kind)
}

#[test]
fn working_state_is_busy() {
    assert_eq!(
        kind(r#"{"state":"working","event":"UserPromptSubmit","ts":1760000000}"#),
        Some(HookKind::Busy)
    );
}

#[test]
fn idle_state_is_idle() {
    assert_eq!(kind(r#"{"state":"idle","ts":1}"#), Some(HookKind::Idle));
}

#[test]
fn starting_state_is_starting() {
    assert_eq!(
        kind(r#"{"state":"starting","ts":1}"#),
        Some(HookKind::Starting)
    );
}

#[test]
fn waiting_input_carries_the_message() {
    let rec = parse(r#"{"state":"waiting_input","message":"Allow rm?","ts":1}"#).unwrap();
    assert_eq!(rec.kind, HookKind::Blocked);
    assert_eq!(rec.message.as_deref(), Some("Allow rm?"));
}

#[test]
fn waiting_input_without_message_has_none() {
    let rec = parse(r#"{"state":"waiting_input","ts":1}"#).unwrap();
    assert_eq!(rec.kind, HookKind::Blocked);
    assert_eq!(rec.message, None);
}

#[test]
fn session_id_is_kept_when_a_string() {
    let rec = parse(r#"{"state":"idle","session_id":"s-1","ts":1}"#).unwrap();
    assert_eq!(rec.session_id.as_deref(), Some("s-1"));
    let rec = parse(r#"{"state":"idle","ts":1}"#).unwrap();
    assert_eq!(rec.session_id, None);
}

#[test]
fn idle_prompt_notification_is_idle_not_blocked() {
    assert_eq!(
        kind(r#"{"state":"waiting_input","notification_type":"idle_prompt","message":"x","ts":1}"#),
        Some(HookKind::Idle)
    );
}

#[test]
fn timestamp_seconds_become_milliseconds() {
    let rec = parse(r#"{"state":"working","ts":1760000000}"#).unwrap();
    assert_eq!(rec.at, Ts::from_millis(1_760_000_000_000));
}

#[test]
fn invalid_json_is_absent() {
    assert_eq!(parse("not json"), None);
    assert_eq!(Claude.parse_hook(&[0xff, 0xfe]), None);
}

#[test]
fn truncated_json_is_absent() {
    assert_eq!(parse(r#"{"state":"working","ts":17600"#), None);
}

#[test]
fn non_object_json_is_absent() {
    assert_eq!(parse("[1,2]"), None);
}

#[test]
fn missing_state_is_absent() {
    assert_eq!(parse(r#"{"ts":1}"#), None);
}

#[test]
fn unknown_state_is_absent() {
    assert_eq!(parse(r#"{"state":"napping","ts":1}"#), None);
}

#[test]
fn missing_ts_is_absent() {
    assert_eq!(parse(r#"{"state":"idle"}"#), None);
}

#[test]
fn string_ts_is_absent() {
    assert_eq!(parse(r#"{"state":"idle","ts":"1760000000"}"#), None);
}

#[test]
fn negative_or_overflowing_ts_is_absent() {
    assert_eq!(parse(r#"{"state":"idle","ts":-1}"#), None);
    assert_eq!(parse(r#"{"state":"idle","ts":9223372036854775807}"#), None);
}

const RULE: &str = "────────────────";

fn prompt(tail: &str) -> Option<String> {
    Claude.pane_prompt(tail)
}

#[test]
fn pane_prompt_is_the_line_above_the_input_box() {
    let tail = format!("Allow rm?\n\n{RULE}\n> \n{RULE}\n  ? for shortcuts");
    assert_eq!(prompt(&tail).as_deref(), Some("Allow rm?"));
    let boxed = "Allow rm?\n╭──────╮\n│ >    │\n╰──────╯\n  esc to interrupt";
    assert_eq!(prompt(boxed).as_deref(), Some("Allow rm?"));
}

#[test]
fn pane_prompt_skips_blank_lines_and_the_footer() {
    let tail = format!("Allow rm?\n  \n\n{RULE}\n> \n{RULE}\n  esc to interrupt");
    assert_eq!(prompt(&tail).as_deref(), Some("Allow rm?"));
}

#[test]
fn pane_prompt_needs_an_input_box_and_text_above_it() {
    assert_eq!(prompt("esc to interrupt"), None);
    assert_eq!(prompt(&format!("{RULE}\n> \n{RULE}\nfooter")), None);
}

fn first_run(tail: &str) -> Option<FirstRunPrompt> {
    Claude.first_run_prompt(tail)
}

#[test]
fn first_run_prompts_are_told_apart() {
    assert_eq!(
        first_run("Do you trust the files in this folder?"),
        Some(FirstRunPrompt::FolderTrust)
    );
    assert_eq!(
        first_run("│ Press Enter to continue │"),
        Some(FirstRunPrompt::PressEnter)
    );
    assert_eq!(
        first_run("Security notes:\n1. It can make mistakes"),
        Some(FirstRunPrompt::SecurityNotes)
    );
    assert_eq!(
        first_run("Choose the text style that looks best with your terminal"),
        Some(FirstRunPrompt::Theme)
    );
    assert_eq!(first_run("❯ 2. Light mode"), Some(FirstRunPrompt::Theme));
}

#[test]
fn ordinary_pane_text_is_no_first_run_prompt() {
    assert_eq!(first_run("● Reading the files\n> "), None);
    assert_eq!(first_run("Yes, proceed"), None);
}
